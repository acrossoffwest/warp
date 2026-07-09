# Hold ⌘Q to Quit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reflex-proof quitting: ⌘Q must be held for 2 seconds (with a progress overlay) before the app terminates; releasing earlier cancels.

**Architecture:** A `cmd-q` default keystroke on the existing `workspace:terminate_app` binding routes ⌘Q into Rust (`performKeyEquivalent:` already short-circuits the menu for assigned keystrokes). The Workspace holds a small `HoldToQuitState`; a self-rescheduling 50ms timer (blink_cursors pattern) polls the physical key state via `CGEventSourceKeyState`/`CGEventSourceFlagsState` (Rust never receives keyUp on macOS, so we poll instead of listening). On 2s of continuous hold → `ctx.terminate_app(TerminationMode::ForceTerminate, None)` (skips the confirm dialog, still flushes persistence). The overlay is a centered pill rendered in `Workspace::render` while the state is active. A `GeneralSettings` toggle (default on) falls back to the old instant `Cancellable` path.

**Tech Stack:** Rust, warpui, CoreGraphics FFI (`#[link(name = "CoreGraphics", kind = "framework")]`), existing settings/binding/overlay machinery.

## Global Constraints

- Menu item "Quit Warp" stays `MenuItem::Standard(StandardAction::Quit)` — mouse click keeps the old `terminate:` → `quit_warning` dialog path.
- Quit after a completed hold must be a clean shutdown: `ctx.terminate_app` only; never `std::process::exit`.
- Hold duration 2.0s, tick 50ms.
- Overlay copy: "Keep holding ⌘Q to quit".
- macOS-only feature; all CG FFI gated `#[cfg(target_os = "macos")]`.
- Setting: `general.hold_cmd_q_to_quit`, default `true`, `SupportedPlatforms::MAC`.

---

### Task 1: Hold state module

**Files:**
- Create: `app/src/hold_to_quit/mod.rs`
- Modify: `app/src/lib.rs` (add `pub mod hold_to_quit;` next to `pub mod quit_warning;`)

**Interfaces:**
- Produces: `HoldToQuitState::new(now: Instant)`, `fn progress(&self, now: Instant) -> f32`, `fn is_complete(&self, now: Instant) -> bool`, consts `HOLD_DURATION: Duration`, `TICK_INTERVAL: Duration`, `fn is_cmd_q_physically_held() -> bool` (mac-gated FFI).

- [ ] **Step 1: Write failing tests** (in-module `#[cfg(test)] mod tests`)

```rust
#[test]
fn progress_goes_from_zero_to_one() {
    let start = Instant::now();
    let state = HoldToQuitState::new(start);
    assert_eq!(state.progress(start), 0.0);
    assert!((state.progress(start + Duration::from_secs(1)) - 0.5).abs() < 0.01);
    assert_eq!(state.progress(start + Duration::from_secs(3)), 1.0);
}

#[test]
fn completes_only_after_full_hold_duration() {
    let start = Instant::now();
    let state = HoldToQuitState::new(start);
    assert!(!state.is_complete(start + Duration::from_millis(1999)));
    assert!(state.is_complete(start + HOLD_DURATION));
}
```

- [ ] **Step 2: Run** `cargo test -p warp --lib hold_to_quit` — FAIL (module missing).

- [ ] **Step 3: Implement**

```rust
use std::time::{Duration, Instant};

pub const HOLD_DURATION: Duration = Duration::from_secs(2);
pub const TICK_INTERVAL: Duration = Duration::from_millis(50);

/// Tracks an in-progress "hold ⌘Q to quit" gesture.
pub struct HoldToQuitState {
    started_at: Instant,
}

impl HoldToQuitState {
    pub fn new(now: Instant) -> Self {
        Self { started_at: now }
    }

    pub fn progress(&self, now: Instant) -> f32 {
        (now.duration_since(self.started_at).as_secs_f32() / HOLD_DURATION.as_secs_f32())
            .clamp(0.0, 1.0)
    }

    pub fn is_complete(&self, now: Instant) -> bool {
        now.duration_since(self.started_at) >= HOLD_DURATION
    }
}

/// Rust receives no keyUp events on macOS, so the hold gesture polls the
/// physical keyboard state instead.
#[cfg(target_os = "macos")]
pub fn is_cmd_q_physically_held() -> bool {
    // kCGEventSourceStateHIDSystemState = 1, kVK_ANSI_Q = 0x0C,
    // kCGEventFlagMaskCommand = 1 << 20.
    const HID_SYSTEM_STATE: u32 = 1;
    const KVK_ANSI_Q: u16 = 0x0C;
    const CMD_FLAG: u64 = 1 << 20;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceKeyState(state: u32, keycode: u16) -> bool;
        fn CGEventSourceFlagsState(state: u32) -> u64;
    }
    unsafe {
        CGEventSourceKeyState(HID_SYSTEM_STATE, KVK_ANSI_Q)
            && CGEventSourceFlagsState(HID_SYSTEM_STATE) & CMD_FLAG != 0
    }
}

#[cfg(not(target_os = "macos"))]
pub fn is_cmd_q_physically_held() -> bool {
    false
}
```

- [ ] **Step 4: Run** `cargo test -p warp --lib hold_to_quit` — PASS.
- [ ] **Step 5: Commit** `feat(quit): hold-to-quit state machine`

### Task 2: Setting + features page toggle

**Files:**
- Modify: `app/src/terminal/general_settings.rs` (after `show_warning_before_quitting` block, ~line 17)
- Modify: `app/src/settings_view/features_page.rs` (toggle list ~line 321, `FeaturesPageAction` enum ~line 620, telemetry match ~line 970)

**Interfaces:**
- Produces: `GeneralSettings::as_ref(ctx).hold_cmd_q_to_quit` (bool, default true).

- [ ] **Step 1: Add setting** — mirror `show_warning_before_quitting` verbatim:

```rust
hold_cmd_q_to_quit: HoldCmdQToQuit {
    type: bool,
    default: true,
    supported_platforms: SupportedPlatforms::MAC,
    sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
    private: false,
    toml_path: "general.hold_cmd_q_to_quit",
    description: "Whether ⌘Q must be held for two seconds to quit Warp.",
},
```

- [ ] **Step 2: Features page** — add `FeaturesPageAction::ToggleHoldCmdQToQuit` variant, its telemetry arm (copy the `ToggleShowWarningBeforeQuitting` arm), its handler (`toggle_and_save_value` on the setting, pattern from the same toggle), and a `ToggleSettingActionPair::new("hold ⌘Q to quit", ...)` push right after the "quit warning modal" pair, including the `.is_supported_on_current_platform(...)` call.
- [ ] **Step 3: Run** `cargo check -p warp` — clean; `cargo test -p warp --lib features_page` if tests exist — PASS.
- [ ] **Step 4: Commit** `feat(quit): hold-to-quit setting`

### Task 3: ⌘Q keystroke + Workspace hold loop

**Files:**
- Modify: `app/src/workspace/mod.rs:932` (binding)
- Modify: `app/src/workspace/view.rs` (field ~line 1041, TerminateApp handler ~line 22612, new methods near `blink`-style helpers)

**Interfaces:**
- Consumes: Task 1 (`HoldToQuitState`, consts, `is_cmd_q_physically_held`), Task 2 (setting).
- Produces: `Workspace.hold_to_quit: Option<HoldToQuitState>`, `hold_to_quit_epoch: usize` (fields Task 4 reads in render).

- [ ] **Step 1: Binding** — add `.with_mac_key_binding("cmd-q")` (API: `crates/warpui_core/src/keymap.rs:688`) to the `workspace:terminate_app` `EditableBinding`.
- [ ] **Step 2: Handler** — replace the body of the `WorkspaceAction::TerminateApp` arm (`view.rs:22612`):

```rust
if cfg!(target_os = "macos") && *GeneralSettings::as_ref(ctx).hold_cmd_q_to_quit {
    self.start_hold_to_quit(ctx);
} else {
    ctx.terminate_app(TerminationMode::Cancellable, None);
}
```

- [ ] **Step 3: Hold loop** — new methods on Workspace (blink_cursors pattern, `view.rs:7420` in editor as the model):

```rust
fn start_hold_to_quit(&mut self, ctx: &mut ViewContext<Self>) {
    if self.hold_to_quit.is_some() {
        return; // key-repeat while already holding
    }
    self.hold_to_quit = Some(HoldToQuitState::new(Instant::now()));
    self.hold_to_quit_epoch += 1;
    self.schedule_hold_to_quit_tick(ctx);
    ctx.notify();
}

fn schedule_hold_to_quit_tick(&mut self, ctx: &mut ViewContext<Self>) {
    let epoch = self.hold_to_quit_epoch;
    let _ = ctx.spawn(
        async move {
            Timer::after(hold_to_quit::TICK_INTERVAL).await;
            epoch
        },
        Self::hold_to_quit_tick,
    );
}

fn hold_to_quit_tick(&mut self, epoch: usize, ctx: &mut ViewContext<Self>) {
    if epoch != self.hold_to_quit_epoch {
        return; // stale timer from a cancelled gesture
    }
    let Some(state) = &self.hold_to_quit else { return };
    if !hold_to_quit::is_cmd_q_physically_held() {
        self.hold_to_quit = None;
        ctx.notify();
        return;
    }
    if state.is_complete(Instant::now()) {
        self.hold_to_quit = None;
        ctx.notify();
        // Holding through the delay is explicit intent: skip the confirm
        // dialog. ForceTerminate still runs the terminate callbacks, so
        // persistence flushes and the session-memory run is marked clean.
        ctx.terminate_app(TerminationMode::ForceTerminate, None);
        return;
    }
    ctx.notify();
    self.schedule_hold_to_quit_tick(ctx);
}
```

- [ ] **Step 4: Run** `cargo check -p warp` — clean. `cargo test -p warp --lib hold_to_quit` — PASS.
- [ ] **Step 5: Commit** `feat(quit): route cmd-q through hold-to-quit loop`

### Task 4: Overlay

**Files:**
- Modify: `app/src/workspace/view.rs` render (`~line 24706`, next to `add_positioned_overlay_child` for `toast_stack`)

**Interfaces:**
- Consumes: `self.hold_to_quit` (Task 3).

- [ ] **Step 1: Render pill** — when `self.hold_to_quit` is `Some`, add a centered overlay: rounded rect (theme background + border like a `DismissibleToast`), label "Keep holding ⌘Q to quit", and a progress bar (filled quad, width = `pill_width * state.progress(Instant::now())`). Position via a `OffsetPositioning` clone of `global_toast_positioning` (`view.rs:20729`) with `PositionedElementAnchor::Middle`/`ChildAnchor::Middle`, zero offset. **Style text fully** (font family id included) — hover/partial styles previously caused a `WrappableText` panic (`warpui_core/ui_components/text.rs:62`).
- [ ] **Step 2: Run** `cargo check -p warp` — clean.
- [ ] **Step 3: Commit** `feat(quit): hold-to-quit progress overlay`

### Task 5: Verification

- [ ] `cargo test -p warp --lib hold_to_quit` and `cargo test -p warp --lib session_memory` — all PASS.
- [ ] `cargo fmt --check` on touched files — clean.
- [ ] `cargo build -p warp --bin warp-oss` — clean.
- [ ] Manual (user): tap ⌘Q → pill flashes, nothing closes; hold 2s → clean quit (next launch: session_memory run marked clean); menu → Quit Warp by mouse → old confirm dialog; toggle setting off → ⌘Q instant (old behavior).
