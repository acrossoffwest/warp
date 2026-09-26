use super::*;
use crate::protocol::{ErrorCode, InputMode};

#[test]
fn running_command_goes_to_pty() {
    assert_eq!(
        route_input(PaneActivity::RunningCommand, false, false),
        Ok(InputRoute::Pty)
    );
}

#[test]
fn alt_screen_goes_to_pty_even_without_running_block() {
    assert_eq!(route_input(PaneActivity::AtPrompt, true, false), Ok(InputRoute::Pty));
}

#[test]
fn prompt_without_allow_shell_is_not_in_tui() {
    let error = route_input(PaneActivity::AtPrompt, false, false).unwrap_err();
    assert_eq!(error.code, ErrorCode::NotInTui);
}

#[test]
fn prompt_with_allow_shell_goes_to_input_editor() {
    assert_eq!(
        route_input(PaneActivity::AtPrompt, false, true),
        Ok(InputRoute::InputEditor)
    );
}

#[test]
fn warp_agent_block_is_busy() {
    let error = route_input(PaneActivity::WarpAgentRunning, false, true).unwrap_err();
    assert_eq!(error.code, ErrorCode::PaneBusy);
}

#[test]
fn paste_wraps_and_normalizes_newlines_when_bracketed() {
    assert_eq!(
        encode_pty_text("a\nb\r\nc", InputMode::Paste, true),
        b"\x1b[200~a\rb\rc\x1b[201~".to_vec()
    );
}

#[test]
fn paste_without_bracketed_mode_is_raw() {
    assert_eq!(encode_pty_text("a\nb", InputMode::Paste, false), b"a\rb".to_vec());
}

#[test]
fn keys_never_wrap() {
    assert_eq!(encode_pty_text("a\nb", InputMode::Keys, true), b"a\rb".to_vec());
}

#[test]
fn encode_empty_text_is_empty() {
    assert!(encode_pty_text("", InputMode::Keys, true).is_empty());
    assert!(encode_pty_text("", InputMode::Paste, false).is_empty());
}
