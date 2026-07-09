# Hold ⌘Q to Quit — Design

Date: 2026-07-09
Status: approved (chat), implementation pending

## Problem

Reflexive ⌘Q kills the whole terminal with live agent sessions. The existing
`quit_warning` confirm dialog does not protect against the reflex: the same
reflex confirms it with Enter. Session Memory recovers the chats afterwards,
but the quit itself should not happen by accident in the first place.

## Decision

Chrome-style hold-to-quit, 2 seconds, always for the ⌘Q keyboard path.
Confirm dialog (existing `quit_warning`) stays for the menu-click path.

Rejected alternatives:
- Confirm dialog for ⌘Q — reflex ⌘Q→Enter defeats it.
- 3s hold — annoying for intentional quits; 2s is enough to break a reflex.
- Hybrid (hold only with live agent sessions) — unpredictable muscle memory.

## Architecture

1. **Key interception (warpui mac platform).** ⌘Q keyDown is intercepted
   before menu keyEquivalent dispatch and does not reach `terminate:`.
   The "Quit Warp" menu item remains; clicking it with the mouse uses the
   old path including the `quit_warning` dialog.

2. **Hold state machine (Rust).** `HoldToQuitState`:
   - first ⌘Q keyDown → start, show overlay, t=0;
   - macOS key-repeat keyDowns confirm the key is still held;
   - keyUp of `q` or release of `⌘` (flagsChanged) → cancel, hide overlay;
   - 2.0s of continuous hold → quit via the existing termination path,
     **bypassing** the confirm dialog (holding = explicit intent).
   Clean shutdown must still run (persistence `ModelEvent::Terminate`),
   so Session Memory records a clean app run.

3. **Overlay.** Centered pill over the active window, Chrome-style:
   "Держи ⌘Q чтобы выйти" + progress bar 0→2s. Appears on first press
   (serves as the warning), disappears on cancel.

4. **Setting.** Boolean `Hold ⌘Q to quit` (default: on) next to the existing
   quit-confirmation setting. Off → ⌘Q behaves as before (menu accelerator →
   `quit_warning` flow).

## Testing

- Unit: state machine — start/repeat/cancel-on-keyup/cancel-on-modifier-release,
  completion at the 2s boundary, disabled-setting passthrough.
- Smoke: overlay renders in both themes.
- Manual: short press does nothing visible except the overlay flash;
  2s hold quits; menu click still shows the confirm dialog.
