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

#[cfg(test)]
mod tests {
    use super::*;

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
}
