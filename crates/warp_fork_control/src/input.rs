use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::protocol::{ErrorBody, ErrorCode, InputMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputRoute {
    Pty,
    InputEditor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneActivity {
    AtPrompt,
    RunningCommand,
    WarpAgentRunning,
}

const BRACKETED_PASTE_START: &str = "\x1b[200~";
const BRACKETED_PASTE_END: &str = "\x1b[201~";

/// Same delay upstream uses before the Enter that submits a bracketed paste to a
/// CLI agent; an Enter arriving together with the paste end marker is dropped.
pub const PASTE_SUBMIT_DELAY: Duration = Duration::from_millis(300);

pub fn route_input(
    activity: PaneActivity,
    is_alt_screen: bool,
    allow_shell: bool,
) -> Result<InputRoute, ErrorBody> {
    if is_alt_screen {
        return Ok(InputRoute::Pty);
    }
    match activity {
        PaneActivity::RunningCommand => Ok(InputRoute::Pty),
        PaneActivity::WarpAgentRunning => Err(ErrorBody::new(
            ErrorCode::PaneBusy,
            "a Warp agent is running in this pane",
        )),
        PaneActivity::AtPrompt if allow_shell => Ok(InputRoute::InputEditor),
        PaneActivity::AtPrompt => Err(ErrorBody::new(
            ErrorCode::NotInTui,
            "pane is at a shell prompt; pass allow_shell to type into the Warp input editor",
        )),
    }
}

pub fn encode_pty_text(text: &str, mode: InputMode, bracketed_paste_enabled: bool) -> Vec<u8> {
    if text.is_empty() {
        return Vec::new();
    }
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    match mode {
        InputMode::Paste if bracketed_paste_enabled => {
            format!("{BRACKETED_PASTE_START}{normalized}{BRACKETED_PASTE_END}").into_bytes()
        }
        _ => normalized.into_bytes(),
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
