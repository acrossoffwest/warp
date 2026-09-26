# Session Restore Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After Warp closes for any reason, the next launch relaunches every Claude Code / Codex session that was running at that moment in its own restored pane, resuming the same conversation, and nothing from earlier runs.

**Architecture:** Each terminal pane writes one session-memory record (`warp_terminal:<pane uuid>`), and the running command decides whether it is an agent record. An agent's exit is written to the database by a targeted UPDATE keyed on the agent block's start second. The upsert never clears that ended mark unless a new agent block starts. At startup, a single pass runs after all snapshot windows exist. It picks the previous app run's still-live agent records and maps each one to its restored pane uuid in any window. It then runs `claude --resume <id>` / `codex resume <id>`, or `--continue` / `resume --last` when no id is known, or inserts that command into the input when the setting is off.

**Tech Stack:** Rust (edition 2024), crate `warp` (`app/`), warpui views/models, diesel + SQLite (`crates/persistence` schema), `cargo test -p warp --lib`.

**Spec:** docs/superpowers/specs/2026-09-26-session-restore-fix-design.md

## Global Constraints

- Work on branch `fix/session-restore`. Do not switch branches.
- Crate under change: `warp` (`app/`), edition 2024. The test command is `cargo test -p warp --lib <filter>`.
- Tests live in sibling `*_tests.rs` files included with `#[cfg(test)] mod <name>_tests;`. Inline `mod tests {` is forbidden (`script/check_no_inline_test_modules`).
- No code comments except a one-line non-obvious invariant. Never reference tickets, plan task numbers or review history in code.
- Commit messages use Conventional Commits. Never add `Co-Authored-By` or any Claude/Anthropic attribution line.
- Never edit files under `crates/persistence/migrations/`. No schema changes; reuse existing `session_memory_records` columns.
- No absolute home-directory paths, usernames or hostnames in tracked files (use `~` or placeholders).
- `.clippy.toml` disallows `std::time::Instant` (use `instant::Instant`) and `std::process::Command`.
- `TerminalView.model` is a non-reentrant `FairMutex`. Take every new `model.lock()` in its own statement or block, never while another lock guard is alive.
- Record id format: `warp_terminal:<base64 STANDARD of pane uuid>`.
- The setting `agents.session_memory.auto_restore_interrupted_sessions` defaults to `true`. `true` runs the resume command; `false` inserts it into the input.
- Session-file start tolerance: `SESSION_FILE_START_TOLERANCE_SECONDS = 2`.
- Resume commands: `claude --resume <id>`, `codex resume <id>`, `claude --continue`, `codex resume --last`. The dangerous flag is appended only for `AgentPermissionMode::Dangerous`.
- Only `LaunchMode::App` inserts rows into `session_memory_app_runs`.

## Review Focus

- A bare `--resume` / `-r` followed by a flag or at end of line (`claude --resume --dangerously-skip-permissions`, `claude --dangerously-skip-permissions --resume`, both present in the fc DB) must yield no id. Test: Task 1 `parse_rejects_flag_or_missing_value_after_resume`.
- A later full-row snapshot upsert of the same agent block must not clear `completed_at`. Test: Task 2 `session_memory_agent_end_survives_later_snapshot_upsert`.
- The record's `started_at` and the ended-UPDATE key must come from one conversion of the same block timestamp, or the UPDATE silently never matches. Test: Task 1 `record_started_at_matches_block_timestamp_seconds`.
- The offered marker must not clobber a row already rewritten by the current run (restored panes reuse their uuid). Test: Task 5 `mark_records_offered_skips_rows_rewritten_by_current_run`.
- Two id-less candidates for the same agent in one cwd must not both run `--continue` into the same conversation. Test: Task 6 `startup_restore_continues_only_newest_idless_candidate_per_cwd`.

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `app/src/session_memory/recording.rs` | create | Pure pane-record builder, command id parsing, record id, block timestamp conversion |
| `app/src/session_memory/recording_tests.rs` | create | Tests for the builder and parsing |
| `app/src/session_memory/types.rs` | modify | `is_agent`, `keep_agent_end`; drop `normalize_terminal_agent_command` |
| `app/src/session_memory/types_tests.rs` | create | Tests for `keep_agent_end` |
| `app/src/session_memory/model.rs` | modify | Candidate selection, offered marker event; drop dedupe, 30-minute rule, normalize, `mark_agent_session_ended*` |
| `app/src/session_memory/model_tests.rs` | modify | Replace old-behavior tests |
| `app/src/session_memory/restore.rs` | modify | Startup resume plan, session-file matching, multi-window planner; drop old startup action API |
| `app/src/session_memory/restore_tests.rs` | modify | New planner tests; drop old startup tests |
| `app/src/session_memory/mod.rs` | modify | Register `recording`, `recording_tests`, `types_tests` |
| `app/src/persistence/mod.rs` | modify | New `ModelEvent` variants; `initialize` gains `record_session_memory_app_run` |
| `app/src/persistence/sqlite.rs` | modify | Ended UPDATE, preserving upsert, offered UPDATE, run-row gating |
| `app/src/persistence/sqlite_tests.rs` | modify | Persistence tests |
| `app/src/lib.rs` | modify | Pass the launch-mode flag to `persistence::initialize`; call the startup pass once after windows exist |
| `app/src/pane_group/pane/terminal_pane.rs` | modify | Use the builder; mark ended on `BlockCompleted`; drop `restored_cwd` |
| `app/src/pane_group/mod.rs` | modify | Drop `restored_terminal_pane_targets` and `restored_cwd` plumbing |
| `app/src/terminal/view.rs` | modify | Deferred run-or-insert restore command; drop native-id ended path |
| `app/src/terminal/view_tests.rs` | modify | Deferred insert test |
| `app/src/settings/ai.rs` | modify | Default `true`, description |
| `app/src/settings/ai_tests.rs` | modify | Default assertion |
| `app/src/workspace/agent_session_reader.rs` | modify | `created_at` on `AgentSessionEntry` |
| `app/src/workspace/view.rs` | modify | Drop per-window restore and enrichment; register `session_memory_startup`; add `terminal_pane_session_uuids` |
| `app/src/workspace/view/session_memory_startup.rs` | create | One-shot startup restore pass |

---

## Task 1: Pane record builder and command id parsing

**Files:**
- Create: `app/src/session_memory/recording.rs`
- Create: `app/src/session_memory/recording_tests.rs`
- Modify: `app/src/session_memory/mod.rs` (lines 1-9)
- Modify: `app/src/session_memory/restore.rs` (line ~202: `fn is_env_assignment` → `pub(super) fn is_env_assignment`)

**Interfaces:**
- Consumes: `types::{terminal_agent_command, user_command, SessionMemoryRecord, ...}`, `crate::terminal::CLIAgent`, `crate::terminal::cli_agent_sessions::CLIAgentSession` (`pub agent`, `pub session_context`), `CLIAgentSessionContext::display_title()` (`pub(crate)`).
- Produces:
  - `pub fn record_id_for_pane_uuid(uuid: &[u8]) -> String`
  - `pub fn block_timestamp_seconds(timestamp: Option<&DateTime<Local>>) -> Option<i64>`
  - `pub fn source_for_cli_agent(agent: CLIAgent) -> Option<SessionMemorySource>`
  - `pub fn parse_agent_session_id(command: &str) -> Option<String>`
  - `pub struct PaneRecordInput<'a> { uuid, cwd, running_command, running_command_started_at, last_command, cli_agent_session, restore_payload, app_run_id, now }`
  - `pub fn pane_session_memory_record(input: PaneRecordInput<'_>) -> SessionMemoryRecord`

- [ ] **Step 1: Register the modules and write the failing tests**

`app/src/session_memory/mod.rs` becomes:

```rust
#![allow(dead_code)]

pub mod model;
#[cfg(test)]
mod model_tests;
pub mod recording;
#[cfg(test)]
mod recording_tests;
pub mod restore;
#[cfg(test)]
mod restore_tests;
pub mod types;
```

Create `app/src/session_memory/recording_tests.rs`:

```rust
use std::path::PathBuf;

use chrono::{Local, TimeZone};

use super::recording::{
    PaneRecordInput, block_timestamp_seconds, pane_session_memory_record,
    parse_agent_session_id, record_id_for_pane_uuid,
};
use super::types::{
    AgentPermissionMode, SessionMemoryKind, SessionMemorySource, SessionMemoryStatus,
};
use crate::terminal::CLIAgent;
use crate::terminal::cli_agent_sessions::{
    CLIAgentInputState, CLIAgentSession, CLIAgentSessionContext, CLIAgentSessionStatus,
};

#[test]
fn parse_reads_claude_resume_forms() {
    assert_eq!(parse_agent_session_id("claude --resume abc").as_deref(), Some("abc"));
    assert_eq!(parse_agent_session_id("claude --resume=abc").as_deref(), Some("abc"));
    assert_eq!(parse_agent_session_id("claude -r abc").as_deref(), Some("abc"));
    assert_eq!(
        parse_agent_session_id("claude --dangerously-skip-permissions --resume abc").as_deref(),
        Some("abc")
    );
    assert_eq!(
        parse_agent_session_id("CLAUDE_CONFIG_DIR=/tmp/x claude --resume abc").as_deref(),
        Some("abc")
    );
}

#[test]
fn parse_rejects_flag_or_missing_value_after_resume() {
    assert_eq!(
        parse_agent_session_id("claude --resume --dangerously-skip-permissions"),
        None
    );
    assert_eq!(
        parse_agent_session_id("claude --dangerously-skip-permissions --resume"),
        None
    );
    assert_eq!(parse_agent_session_id("claude -r"), None);
    assert_eq!(parse_agent_session_id("claude --resume="), None);
    assert_eq!(parse_agent_session_id("claude"), None);
}

#[test]
fn parse_reads_codex_resume_forms() {
    assert_eq!(parse_agent_session_id("codex resume abc").as_deref(), Some("abc"));
    assert_eq!(
        parse_agent_session_id("codex resume abc --dangerously-bypass-approvals-and-sandbox")
            .as_deref(),
        Some("abc")
    );
    assert_eq!(
        parse_agent_session_id("codex --dangerously-bypass-approvals-and-sandbox resume abc")
            .as_deref(),
        Some("abc")
    );
    assert_eq!(parse_agent_session_id("codex resume --last"), None);
    assert_eq!(parse_agent_session_id("codex resume"), None);
    assert_eq!(parse_agent_session_id("codex"), None);
}

#[test]
fn parse_ignores_other_programs() {
    assert_eq!(parse_agent_session_id("vim --resume abc"), None);
    assert_eq!(parse_agent_session_id(""), None);
}

#[test]
fn running_claude_without_plugin_records_agent_with_parsed_id() {
    let record = pane_session_memory_record(input(
        Some("claude --resume abc --dangerously-skip-permissions"),
        Some(1_000),
        None,
        None,
    ));

    assert_eq!(record.id, record_id_for_pane_uuid(&[1, 2, 3, 4]));
    assert_eq!(record.source, SessionMemorySource::ClaudeCode);
    assert_eq!(record.kind, SessionMemoryKind::AgentChat);
    assert_eq!(record.status, SessionMemoryStatus::Live);
    assert_eq!(record.native_session_id.as_deref(), Some("abc"));
    assert_eq!(record.permission_mode, AgentPermissionMode::Dangerous);
    assert_eq!(record.started_at, Some(1_000));
    assert_eq!(record.completed_at, None);
    assert_eq!(
        record.launch_argv,
        Some(vec![
            "claude".to_string(),
            "--resume".to_string(),
            "abc".to_string(),
            "--dangerously-skip-permissions".to_string(),
        ])
    );
    assert_eq!(record.terminal_pane_uuid, Some(vec![1, 2, 3, 4]));
    assert_eq!(record.app_run_id.as_deref(), Some("current-run"));
}

#[test]
fn agent_turn_states_all_record_live() {
    for status in [
        CLIAgentSessionStatus::InProgress,
        CLIAgentSessionStatus::Success,
        CLIAgentSessionStatus::Blocked { message: None },
        CLIAgentSessionStatus::Failed {
            error_type: None,
            message: None,
        },
        CLIAgentSessionStatus::Cancelled,
    ] {
        let session = plugin_session(CLIAgent::Claude, status.clone(), Some("plugin-id"));
        let record =
            pane_session_memory_record(input(Some("claude"), Some(10), None, Some(&session)));

        assert_eq!(record.status, SessionMemoryStatus::Live, "{status:?}");
        assert_eq!(record.completed_at, None, "{status:?}");
    }
}

#[test]
fn plugin_session_id_wins_over_command_id() {
    let session = plugin_session(
        CLIAgent::Claude,
        CLIAgentSessionStatus::InProgress,
        Some("plugin-id"),
    );
    let record = pane_session_memory_record(input(
        Some("claude --resume command-id"),
        Some(10),
        None,
        Some(&session),
    ));

    assert_eq!(record.native_session_id.as_deref(), Some("plugin-id"));
    assert_eq!(record.cwd, Some(PathBuf::from("/tmp/plugin-cwd")));
}

#[test]
fn plugin_session_with_alias_command_records_agent() {
    let session = plugin_session(CLIAgent::Codex, CLIAgentSessionStatus::InProgress, None);
    let record = pane_session_memory_record(input(Some("cx"), Some(10), None, Some(&session)));

    assert_eq!(record.source, SessionMemorySource::Codex);
    assert_eq!(record.kind, SessionMemoryKind::AgentChat);
    assert_eq!(record.permission_mode, AgentPermissionMode::Unknown);
    assert_eq!(record.native_session_id, None);
}

#[test]
fn restored_agent_block_without_running_command_records_terminal() {
    let record = pane_session_memory_record(input(
        None,
        None,
        Some("claude --resume abc --dangerously-skip-permissions"),
        None,
    ));

    assert_eq!(record.source, SessionMemorySource::WarpTerminal);
    assert_eq!(record.kind, SessionMemoryKind::Terminal);
    assert_eq!(record.native_session_id, None);
    assert_eq!(record.permission_mode, AgentPermissionMode::Unknown);
    assert_eq!(record.started_at, None);
    assert_eq!(
        record.last_command.as_deref(),
        Some("claude --resume abc --dangerously-skip-permissions")
    );
}

#[test]
fn running_non_agent_command_records_terminal() {
    let record = pane_session_memory_record(input(Some("vim notes.md"), Some(10), None, None));

    assert_eq!(record.source, SessionMemorySource::WarpTerminal);
    assert_eq!(record.kind, SessionMemoryKind::Terminal);
    assert_eq!(record.started_at, None);
}

#[test]
fn record_started_at_matches_block_timestamp_seconds() {
    let block_start = Local.timestamp_millis_opt(1_700_000_000_900).unwrap();
    let started_at = block_timestamp_seconds(Some(&block_start));

    let record = pane_session_memory_record(input(Some("claude"), started_at, None, None));

    assert_eq!(record.started_at, Some(1_700_000_000));
    assert_eq!(record.started_at, block_timestamp_seconds(Some(&block_start)));
}

fn input<'a>(
    running_command: Option<&str>,
    running_command_started_at: Option<i64>,
    last_command: Option<&str>,
    cli_agent_session: Option<&'a CLIAgentSession>,
) -> PaneRecordInput<'a> {
    PaneRecordInput {
        uuid: &[1, 2, 3, 4],
        cwd: Some(PathBuf::from("/tmp/pane-cwd")),
        running_command: running_command.map(str::to_owned),
        running_command_started_at,
        last_command: last_command.map(str::to_owned),
        cli_agent_session,
        restore_payload: None,
        app_run_id: Some("current-run".to_string()),
        now: 2_000,
    }
}

fn plugin_session(
    agent: CLIAgent,
    status: CLIAgentSessionStatus,
    session_id: Option<&str>,
) -> CLIAgentSession {
    CLIAgentSession {
        agent,
        status,
        session_context: CLIAgentSessionContext {
            session_id: session_id.map(str::to_owned),
            cwd: Some("/tmp/plugin-cwd".to_string()),
            ..Default::default()
        },
        input_state: CLIAgentInputState::Closed,
        should_auto_toggle_input: false,
        listener: None,
        plugin_version: None,
        remote_host: None,
        draft_text: None,
        custom_command_prefix: None,
        received_rich_notification: false,
    }
}
```

- [ ] **Step 2: Run the tests (expect failure)**

Run: `cargo test -p warp --lib session_memory::recording`
Expected: compile error `unresolved import super::recording` / `file not found for module recording`.

- [ ] **Step 3: Implement `recording.rs`**

In `app/src/session_memory/restore.rs` change `fn is_env_assignment(token: &str) -> bool {` to `pub(super) fn is_env_assignment(token: &str) -> bool {`.

Create `app/src/session_memory/recording.rs`:

```rust
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use chrono::{DateTime, Local};

use super::restore::is_env_assignment;
use super::types::{
    AgentPermissionMode, SessionMemoryKind, SessionMemoryRecord, SessionMemorySource,
    SessionMemoryStatus, terminal_agent_command, user_command,
};
use crate::terminal::CLIAgent;
use crate::terminal::cli_agent_sessions::CLIAgentSession;

pub fn record_id_for_pane_uuid(uuid: &[u8]) -> String {
    format!("warp_terminal:{}", BASE64_STANDARD.encode(uuid))
}

pub fn block_timestamp_seconds(timestamp: Option<&DateTime<Local>>) -> Option<i64> {
    timestamp.map(DateTime::timestamp)
}

pub fn source_for_cli_agent(agent: CLIAgent) -> Option<SessionMemorySource> {
    match agent {
        CLIAgent::Claude => Some(SessionMemorySource::ClaudeCode),
        CLIAgent::Codex => Some(SessionMemorySource::Codex),
        _ => None,
    }
}

pub fn parse_agent_session_id(command: &str) -> Option<String> {
    let tokens = command
        .split_whitespace()
        .skip_while(|token| is_env_assignment(token))
        .collect::<Vec<_>>();
    let (program, args) = tokens.split_first()?;
    match *program {
        "claude" => claude_session_id(args),
        "codex" => codex_session_id(args),
        _ => None,
    }
}

fn claude_session_id(args: &[&str]) -> Option<String> {
    args.iter().enumerate().find_map(|(index, arg)| {
        if let Some(value) = arg.strip_prefix("--resume=") {
            return session_id_value(value);
        }
        if matches!(*arg, "--resume" | "-r") {
            return args.get(index + 1).and_then(|value| session_id_value(value));
        }
        None
    })
}

fn codex_session_id(args: &[&str]) -> Option<String> {
    let resume_index = args.iter().position(|arg| *arg == "resume")?;
    let resume_args = &args[resume_index + 1..];
    if resume_args.contains(&"--last") {
        return None;
    }
    resume_args.first().and_then(|value| session_id_value(value))
}

fn session_id_value(value: &str) -> Option<String> {
    (!value.is_empty() && !value.starts_with('-')).then(|| value.to_owned())
}

pub struct PaneRecordInput<'a> {
    pub uuid: &'a [u8],
    pub cwd: Option<PathBuf>,
    pub running_command: Option<String>,
    pub running_command_started_at: Option<i64>,
    pub last_command: Option<String>,
    pub cli_agent_session: Option<&'a CLIAgentSession>,
    pub restore_payload: Option<serde_json::Value>,
    pub app_run_id: Option<String>,
    pub now: i64,
}

pub fn pane_session_memory_record(input: PaneRecordInput<'_>) -> SessionMemoryRecord {
    let running_command = user_command(input.running_command.as_deref());
    let agent = running_command
        .as_deref()
        .and_then(|command| running_agent(command, input.cli_agent_session));
    let mut record = SessionMemoryRecord {
        id: record_id_for_pane_uuid(input.uuid),
        source: SessionMemorySource::WarpTerminal,
        kind: SessionMemoryKind::Terminal,
        status: SessionMemoryStatus::Live,
        title: input
            .last_command
            .clone()
            .or_else(|| {
                input
                    .cwd
                    .as_ref()
                    .map(|cwd| cwd.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "Terminal".to_string()),
        summary: None,
        cwd: input.cwd,
        project: None,
        native_session_id: None,
        transcript_path: None,
        terminal_pane_uuid: Some(input.uuid.to_vec()),
        app_window_fingerprint: None,
        app_tab_fingerprint: None,
        last_command: input.last_command,
        last_exit_code: None,
        launch_argv: None,
        permission_mode: AgentPermissionMode::Unknown,
        last_seen_at: input.now,
        started_at: None,
        completed_at: None,
        closed_intentionally_at: None,
        app_run_id: input.app_run_id,
        recovery_offered_run_id: None,
        restore_payload: input.restore_payload,
    };

    let (Some(command), Some((source, permission_mode))) = (running_command, agent) else {
        return record;
    };
    let context = input
        .cli_agent_session
        .filter(|session| source_for_cli_agent(session.agent) == Some(source))
        .map(|session| &session.session_context);

    record.source = source;
    record.kind = SessionMemoryKind::AgentChat;
    record.title = context
        .and_then(|context| context.display_title())
        .unwrap_or_else(|| command.clone());
    record.summary = context.and_then(|context| context.summary.clone());
    if let Some(cwd) = context.and_then(|context| context.cwd.clone()) {
        record.cwd = Some(PathBuf::from(cwd));
    }
    record.project = context.and_then(|context| context.project.clone());
    record.native_session_id = context
        .and_then(|context| context.session_id.clone())
        .or_else(|| parse_agent_session_id(&command));
    record.transcript_path = context
        .and_then(|context| context.transcript_path.as_ref())
        .map(PathBuf::from);
    record.launch_argv = Some(command.split_whitespace().map(str::to_owned).collect());
    record.permission_mode = permission_mode;
    record.started_at = input.running_command_started_at;
    record.last_command = Some(command);
    record
}

fn running_agent(
    command: &str,
    cli_agent_session: Option<&CLIAgentSession>,
) -> Option<(SessionMemorySource, AgentPermissionMode)> {
    if let Some(agent_command) = terminal_agent_command(Some(command)) {
        return Some((agent_command.source, agent_command.permission_mode));
    }
    let source = cli_agent_session.and_then(|session| source_for_cli_agent(session.agent))?;
    Some((source, AgentPermissionMode::Unknown))
}
```

- [ ] **Step 4: Run the tests (expect pass)**

Run: `cargo test -p warp --lib session_memory::recording`
Expected: all 11 tests pass.

- [ ] **Step 5: Commit**

```bash
git add app/src/session_memory/mod.rs app/src/session_memory/recording.rs app/src/session_memory/recording_tests.rs app/src/session_memory/restore.rs
git commit -m "feat(session-memory): build pane records from the running command"
```

---

## Task 2: Persist agent end and keep it across snapshot upserts

**Files:**
- Modify: `app/src/session_memory/types.rs` (`impl SessionMemoryRecord`, ~line 184)
- Create: `app/src/session_memory/types_tests.rs`
- Modify: `app/src/session_memory/mod.rs` (add `#[cfg(test)] mod types_tests;` after `pub mod types;`)
- Modify: `app/src/persistence/mod.rs` (`enum ModelEvent`, ~line 512-521)
- Modify: `app/src/persistence/sqlite.rs` (`handle_model_event` ~878-890, `upsert_session_memory_record` ~1097, new fn after `mark_session_memory_record_closed` ~1128)
- Modify: `app/src/persistence/sqlite_tests.rs` (session memory section, after ~line 1318)

**Interfaces:**
- Produces:
  - `SessionMemoryRecord::is_agent(&self) -> bool`
  - `SessionMemoryRecord::keep_agent_end(&mut self, existing_started_at: Option<i64>, existing_completed_at: Option<i64>)`
  - `ModelEvent::MarkSessionMemoryAgentEnded { id: String, started_at: i64, completed_at: i64 }`
  - `fn mark_session_memory_agent_ended(conn: &mut SqliteConnection, record_id: &str, started_at: i64, completed_at: i64) -> Result<()>` (sqlite.rs, private)
- Consumed by: Task 4 (pane sends the event).

- [ ] **Step 1: Write the failing tests**

`app/src/session_memory/mod.rs`: add after `pub mod types;`:

```rust
#[cfg(test)]
mod types_tests;
```

Create `app/src/session_memory/types_tests.rs`:

```rust
use std::path::PathBuf;

use super::types::{
    AgentPermissionMode, SessionMemoryKind, SessionMemoryRecord, SessionMemorySource,
    SessionMemoryStatus,
};

#[test]
fn stale_snapshot_of_ended_agent_keeps_end() {
    let mut incoming = agent_record(Some(100));

    incoming.keep_agent_end(Some(100), Some(150));

    assert_eq!(incoming.completed_at, Some(150));
    assert_eq!(incoming.status, SessionMemoryStatus::Success);
}

#[test]
fn new_agent_start_clears_previous_end() {
    let mut incoming = agent_record(Some(200));

    incoming.keep_agent_end(Some(100), Some(150));

    assert_eq!(incoming.completed_at, None);
    assert_eq!(incoming.status, SessionMemoryStatus::Live);
}

#[test]
fn new_agent_after_terminal_snapshot_clears_previous_end() {
    let mut incoming = agent_record(Some(200));

    incoming.keep_agent_end(None, Some(150));

    assert_eq!(incoming.completed_at, None);
}

#[test]
fn terminal_snapshot_after_agent_end_keeps_end() {
    let mut incoming = agent_record(None);
    incoming.source = SessionMemorySource::WarpTerminal;
    incoming.kind = SessionMemoryKind::Terminal;

    incoming.keep_agent_end(Some(100), Some(150));

    assert_eq!(incoming.completed_at, Some(150));
    assert_eq!(incoming.status, SessionMemoryStatus::Live);
}

#[test]
fn record_without_existing_end_is_unchanged() {
    let mut incoming = agent_record(Some(100));

    incoming.keep_agent_end(Some(100), None);

    assert_eq!(incoming.completed_at, None);
    assert_eq!(incoming.status, SessionMemoryStatus::Live);
}

fn agent_record(started_at: Option<i64>) -> SessionMemoryRecord {
    SessionMemoryRecord {
        id: "warp_terminal:AQIDBA==".to_string(),
        source: SessionMemorySource::ClaudeCode,
        kind: SessionMemoryKind::AgentChat,
        status: SessionMemoryStatus::Live,
        title: "claude".to_string(),
        summary: None,
        cwd: Some(PathBuf::from("/tmp/session-memory")),
        project: None,
        native_session_id: None,
        transcript_path: None,
        terminal_pane_uuid: Some(vec![1, 2, 3, 4]),
        app_window_fingerprint: None,
        app_tab_fingerprint: None,
        last_command: Some("claude".to_string()),
        last_exit_code: None,
        launch_argv: Some(vec!["claude".to_string()]),
        permission_mode: AgentPermissionMode::Normal,
        last_seen_at: 160,
        started_at,
        completed_at: None,
        closed_intentionally_at: None,
        app_run_id: Some("current-run".to_string()),
        recovery_offered_run_id: None,
        restore_payload: None,
    }
}
```

Append to `app/src/persistence/sqlite_tests.rs` (after `session_memory_record_upsert_clears_previously_set_optional_fields`):

```rust
#[test]
fn session_memory_agent_end_survives_later_snapshot_upsert() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut conn = setup_database(&tempdir.path().join("warp.sqlite"))
        .expect("database should initialize");
    let record = session_memory_agent_record(Some(100));

    upsert_session_memory(&mut conn, record.clone());
    handle_model_event(
        ModelEvent::MarkSessionMemoryAgentEnded {
            id: record.id.clone(),
            started_at: 100,
            completed_at: 150,
        },
        &mut conn,
    )
    .expect("agent end should be written");
    upsert_session_memory(&mut conn, record);

    let stored = read_single_session_memory_record(&mut conn);
    assert_eq!(stored.completed_at, Some(150));
    assert_eq!(stored.status, SessionMemoryStatus::Success);
}

#[test]
fn session_memory_new_agent_start_clears_previous_end() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut conn = setup_database(&tempdir.path().join("warp.sqlite"))
        .expect("database should initialize");
    let record = session_memory_agent_record(Some(100));

    upsert_session_memory(&mut conn, record.clone());
    handle_model_event(
        ModelEvent::MarkSessionMemoryAgentEnded {
            id: record.id.clone(),
            started_at: 100,
            completed_at: 150,
        },
        &mut conn,
    )
    .expect("agent end should be written");
    upsert_session_memory(&mut conn, session_memory_agent_record(Some(200)));

    let stored = read_single_session_memory_record(&mut conn);
    assert_eq!(stored.completed_at, None);
    assert_eq!(stored.status, SessionMemoryStatus::Live);
}

#[test]
fn session_memory_agent_end_ignores_other_block_start() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut conn = setup_database(&tempdir.path().join("warp.sqlite"))
        .expect("database should initialize");
    let record = session_memory_agent_record(Some(100));

    upsert_session_memory(&mut conn, record.clone());
    handle_model_event(
        ModelEvent::MarkSessionMemoryAgentEnded {
            id: record.id,
            started_at: 99,
            completed_at: 150,
        },
        &mut conn,
    )
    .expect("agent end event should be handled");

    let stored = read_single_session_memory_record(&mut conn);
    assert_eq!(stored.completed_at, None);
    assert_eq!(stored.status, SessionMemoryStatus::Live);
}

fn session_memory_agent_record(started_at: Option<i64>) -> SessionMemoryRecord {
    SessionMemoryRecord {
        id: "warp_terminal:AQIDBA==".to_string(),
        source: SessionMemorySource::ClaudeCode,
        kind: SessionMemoryKind::AgentChat,
        status: SessionMemoryStatus::Live,
        title: "claude".to_string(),
        summary: None,
        cwd: Some(PathBuf::from("/tmp/warp-session-memory-test")),
        project: None,
        native_session_id: Some("abc".to_string()),
        transcript_path: None,
        terminal_pane_uuid: Some(vec![1, 2, 3, 4]),
        app_window_fingerprint: None,
        app_tab_fingerprint: None,
        last_command: Some("claude --resume abc".to_string()),
        last_exit_code: None,
        launch_argv: None,
        permission_mode: AgentPermissionMode::Normal,
        last_seen_at: 160,
        started_at,
        completed_at: None,
        closed_intentionally_at: None,
        app_run_id: Some("current-run".to_string()),
        recovery_offered_run_id: None,
        restore_payload: None,
    }
}

fn upsert_session_memory(conn: &mut diesel::sqlite::SqliteConnection, record: SessionMemoryRecord) {
    handle_model_event(ModelEvent::UpsertSessionMemoryRecord { record }, conn)
        .expect("session memory record should upsert");
}

fn read_single_session_memory_record(
    conn: &mut diesel::sqlite::SqliteConnection,
) -> SessionMemoryRecord {
    read_sqlite_data(conn, None, PersistedDataScope::Full)
        .expect("app state should load")
        .session_memory_records
        .pop()
        .expect("record should exist")
}
```

- [ ] **Step 2: Run the tests (expect failure)**

Run: `cargo test -p warp --lib session_memory`
Expected: compile errors `no method named keep_agent_end` and `no variant named MarkSessionMemoryAgentEnded`.

- [ ] **Step 3: Implement**

`app/src/session_memory/types.rs`: inside `impl SessionMemoryRecord`, add before `pub fn is_interrupted`:

```rust
    pub fn is_agent(&self) -> bool {
        matches!(
            self.source,
            SessionMemorySource::ClaudeCode | SessionMemorySource::Codex
        )
    }

    pub fn keep_agent_end(
        &mut self,
        existing_started_at: Option<i64>,
        existing_completed_at: Option<i64>,
    ) {
        let Some(completed_at) = existing_completed_at else {
            return;
        };
        if self.completed_at.is_some() {
            return;
        }
        let new_agent_started = self.is_agent()
            && self.started_at.is_some()
            && self.started_at != existing_started_at;
        if new_agent_started {
            return;
        }
        self.completed_at = Some(completed_at);
        if self.is_agent() {
            self.status = SessionMemoryStatus::Success;
        }
    }
```

`app/src/persistence/mod.rs`, in `enum ModelEvent` after `MarkSessionMemoryRecordClosed { .. },`:

```rust
    MarkSessionMemoryAgentEnded {
        id: String,
        started_at: i64,
        completed_at: i64,
    },
```

`app/src/persistence/sqlite.rs`, in `handle_model_event` after the `MarkSessionMemoryRecordClosed` arm:

```rust
        ModelEvent::MarkSessionMemoryAgentEnded {
            id,
            started_at,
            completed_at,
        } => mark_session_memory_agent_ended(connection, &id, started_at, completed_at)
            .context("error marking session memory agent ended"),
```

Replace `upsert_session_memory_record` with:

```rust
fn upsert_session_memory_record(
    conn: &mut SqliteConnection,
    mut record: SessionMemoryRecord,
) -> Result<()> {
    let existing = schema::session_memory_records::dsl::session_memory_records
        .filter(schema::session_memory_records::dsl::id.eq(&record.id))
        .select((
            schema::session_memory_records::dsl::started_at,
            schema::session_memory_records::dsl::completed_at,
        ))
        .first::<(Option<i64>, Option<i64>)>(conn)
        .optional()?;
    if let Some((existing_started_at, existing_completed_at)) = existing {
        record.keep_agent_end(existing_started_at, existing_completed_at);
    }

    let row = session_memory_record_to_db(record)?;
    diesel::insert_into(schema::session_memory_records::dsl::session_memory_records)
        .values(&row)
        .on_conflict(schema::session_memory_records::dsl::id)
        .do_update()
        .set(&row)
        .execute(conn)?;
    Ok(())
}
```

Add after `mark_session_memory_record_closed`:

```rust
fn mark_session_memory_agent_ended(
    conn: &mut SqliteConnection,
    record_id: &str,
    started_at: i64,
    completed_at: i64,
) -> Result<()> {
    diesel::update(
        schema::session_memory_records::dsl::session_memory_records
            .filter(schema::session_memory_records::dsl::id.eq(record_id))
            .filter(schema::session_memory_records::dsl::started_at.eq(started_at))
            .filter(schema::session_memory_records::dsl::completed_at.is_null())
            .filter(
                schema::session_memory_records::dsl::source
                    .ne(session_memory_source_to_db(SessionMemorySource::WarpTerminal)),
            ),
    )
    .set((
        schema::session_memory_records::dsl::completed_at.eq(completed_at),
        schema::session_memory_records::dsl::status
            .eq(session_memory_status_to_db(SessionMemoryStatus::Success)),
    ))
    .execute(conn)?;
    Ok(())
}
```

- [ ] **Step 4: Run the tests (expect pass)**

Run: `cargo test -p warp --lib session_memory`
Expected: the 5 `types_tests` pass, the 3 new sqlite tests pass, and the existing `session_memory_records_round_trip_and_lifecycle_events` and `session_memory_record_upsert_clears_previously_set_optional_fields` still pass.

- [ ] **Step 5: Commit**

```bash
git add app/src/session_memory/types.rs app/src/session_memory/types_tests.rs app/src/session_memory/mod.rs app/src/persistence/mod.rs app/src/persistence/sqlite.rs app/src/persistence/sqlite_tests.rs
git commit -m "fix(session-memory): persist agent end and keep it across snapshots"
```

---

## Task 3: Only the GUI app creates app-run rows

**Files:**
- Modify: `app/src/persistence/sqlite.rs` (`initialize` ~133-165, new fn before `start_session_memory_app_run` ~968)
- Modify: `app/src/persistence/mod.rs` (`initialize` ~169-184)
- Modify: `app/src/lib.rs` (~1621-1622)
- Modify: `app/src/persistence/sqlite_tests.rs` (imports lines 14-19 and after `session_memory_app_run_tracks_recoverable_previous_run`)

**Interfaces:**
- Produces:
  - `pub fn initialize(ctx: &mut AppContext, scope: PersistenceScope, data_scope: PersistedDataScope, record_session_memory_app_run: bool) -> (Option<Box<PersistedData>>, Option<WriterHandles>)` (same signature in `persistence::initialize` and `sqlite::initialize`)
  - `fn begin_session_memory_run(conn: &mut SqliteConnection, record_app_run: bool) -> SessionMemoryRunState` (sqlite.rs, private, visible to `sqlite_tests`)

- [ ] **Step 1: Write the failing tests**

In `app/src/persistence/sqlite_tests.rs`, add `begin_session_memory_run` to the `use super::{ ... }` list. Add these imports:

```rust
use diesel::{QueryDsl, RunQueryDsl};
```

Append:

```rust
#[test]
fn non_app_launch_does_not_create_session_memory_app_run() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut conn = setup_database(&tempdir.path().join("warp.sqlite"))
        .expect("database should initialize");

    let run_state = begin_session_memory_run(&mut conn, false);

    assert_eq!(run_state.previous_run_id, None);
    assert_eq!(session_memory_app_run_count(&mut conn), 0);
}

#[test]
fn app_launch_creates_one_session_memory_app_run() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut conn = setup_database(&tempdir.path().join("warp.sqlite"))
        .expect("database should initialize");

    let first = begin_session_memory_run(&mut conn, true);
    let second = begin_session_memory_run(&mut conn, true);

    assert_eq!(session_memory_app_run_count(&mut conn), 2);
    assert_eq!(
        second.previous_run_id.as_deref(),
        Some(first.current_run_id.as_str())
    );
}

fn session_memory_app_run_count(conn: &mut diesel::sqlite::SqliteConnection) -> i64 {
    crate::persistence::schema::session_memory_app_runs::dsl::session_memory_app_runs
        .count()
        .get_result(conn)
        .expect("app runs should be counted")
}
```

- [ ] **Step 2: Run the tests (expect failure)**

Run: `cargo test -p warp --lib session_memory_app_run`
Expected: compile error `unresolved import super::begin_session_memory_run`.

- [ ] **Step 3: Implement**

`app/src/persistence/sqlite.rs`: add before `start_session_memory_app_run`:

```rust
fn begin_session_memory_run(
    conn: &mut SqliteConnection,
    record_app_run: bool,
) -> SessionMemoryRunState {
    if !record_app_run {
        return SessionMemoryRunState::test_default();
    }
    match start_session_memory_app_run(conn) {
        Ok(run_state) => run_state,
        Err(err) => {
            report_error!(err.context("Failed to start session memory app run"));
            SessionMemoryRunState::test_default()
        }
    }
}
```

In `sqlite::initialize`, add the parameter `record_session_memory_app_run: bool` after `data_scope: PersistedDataScope`. Replace the block from `let session_memory_run_state = match start_session_memory_app_run(&mut conn) {` through `Some(session_memory_current_run_id),` in the `start_writer` call with:

```rust
            let session_memory_run_state =
                begin_session_memory_run(&mut conn, record_session_memory_app_run);
            let session_memory_run_id = record_session_memory_app_run
                .then(|| session_memory_run_state.current_run_id.clone());
            let mut persisted_data = read_persisted_data(&mut conn, ctx, data_scope);
            if let Some(persisted_data) = persisted_data.as_mut() {
                persisted_data.session_memory_run_state = session_memory_run_state;
            }

            let writer_handles = match start_writer(
                conn,
                database_path.clone(),
                session_memory_run_id,
            ) {
```

`app/src/persistence/mod.rs` `initialize`: add the parameter `record_session_memory_app_run: bool` after `data_scope: PersistedDataScope`. Change the inner call to `sqlite::initialize(ctx, scope, data_scope, record_session_memory_app_run)`. The existing `#[cfg_attr(not(feature = "local_fs"), allow(unused_variables))]` covers the non-`local_fs` branch.

`app/src/lib.rs` (~1621):

```rust
    let (sqlite_data, writer_handles) = persistence::initialize(
        ctx,
        persistence_scope,
        persisted_data_scope,
        matches!(launch_mode, LaunchMode::App { .. }),
    );
```

- [ ] **Step 4: Run the tests (expect pass)**

Run: `cargo test -p warp --lib session_memory_app_run` and then `cargo check -p warp`.
Expected: both new tests and `session_memory_app_run_tracks_recoverable_previous_run` pass. `cargo check` is clean.

- [ ] **Step 5: Commit**

```bash
git add app/src/persistence/sqlite.rs app/src/persistence/mod.rs app/src/lib.rs app/src/persistence/sqlite_tests.rs
git commit -m "fix(session-memory): record app runs only for the GUI app"
```

---

## Task 4: Wire panes to the builder and write agent end on block completion

**Files:**
- Modify: `app/src/pane_group/pane/terminal_pane.rs` (helpers ~123-232, `upsert_session_memory_record` ~380-466, `attach` CLI agent subscription ~548-587, `snapshot` ~845-849, `handle_terminal_view_event` `BlockCompleted` arm ~1280, imports ~56-80)
- Modify: `app/src/terminal/view.rs` (~12478-12496)
- Modify: `app/src/session_memory/model.rs` (remove `mark_agent_session_ended`, `mark_agent_session_ended_and_notify`, `mark_agent_session_ended_for_native_session_and_notify`)
- Modify: `app/src/session_memory/model_tests.rs` (remove `mark_agent_session_ended_sets_completed_at`)

**Interfaces:**
- Consumes: Task 1 `pane_session_memory_record`, `PaneRecordInput`, `record_id_for_pane_uuid`, `block_timestamp_seconds`. Task 2 `ModelEvent::MarkSessionMemoryAgentEnded`. `TerminalModel::block_list()`, `BlockList::active_block()`, `Block::{is_active_and_long_running, command_to_string, start_ts}`, `TerminalView::{pwd_if_local, shell_launch_data_if_local, session_command_context}`, `ctx.windows().stage()`, `warpui::windowing::state::ApplicationStage`.
- Produces:
  - `fn session_memory_record_for_pane(uuid: &[u8], terminal_view: &ViewHandle<TerminalView>, ctx: &AppContext) -> SessionMemoryRecord` (terminal_pane.rs, private)
  - `TerminalPane::mark_session_memory_agent_ended(&self, block: &SerializedBlock, ctx: &AppContext)` (`pub(in crate::pane_group)`)

- [ ] **Step 1: Tests**

No new unit test in this task. The record content is covered by the Task 1 builder tests. Persistence of the end is covered by the Task 2 tests. The key agreement is covered by `record_started_at_matches_block_timestamp_seconds`. This task only moves view data into those tested functions. It is verified by compilation, the existing suite, and the Task 10 manual scenario. Remove `mark_agent_session_ended_sets_completed_at` from `model_tests.rs`, because the method it tests is deleted. The in-memory-only end never reached the database, and the pane-level UPDATE replaces it.

- [ ] **Step 2: Confirm the current state compiles**

Run: `cargo check -p warp`
Expected: clean. This is the baseline before editing.

- [ ] **Step 3: Implement**

In `terminal_pane.rs`:

1. Delete `session_memory_record_id_for_uuid`, `session_memory_source_for_cli_agent`, `session_memory_status_for_cli_agent` and `cli_agent_session_memory_record` (~144-232).
2. Add in their place:

```rust
fn session_memory_record_for_pane(
    uuid: &[u8],
    terminal_view: &ViewHandle<TerminalView>,
    ctx: &AppContext,
) -> SessionMemoryRecord {
    let view = terminal_view.as_ref(ctx);
    let (running_command, running_command_started_at) = {
        let model = view.model.lock();
        let active_block = model.block_list().active_block();
        if active_block.is_active_and_long_running() {
            (
                Some(active_block.command_to_string()),
                block_timestamp_seconds(active_block.start_ts()),
            )
        } else {
            (None, None)
        }
    };
    let restore_payload = view
        .shell_launch_data_if_local(ctx)
        .and_then(|shell_launch_data| serde_json::to_value(shell_launch_data).ok())
        .map(|shell_launch_data| serde_json::json!({ "shell_launch_data": shell_launch_data }));

    pane_session_memory_record(PaneRecordInput {
        uuid,
        cwd: view.pwd_if_local(ctx).map(PathBuf::from),
        running_command,
        running_command_started_at,
        last_command: terminal_last_command(view.session_command_context(ctx)),
        cli_agent_session: CLIAgentSessionsModel::as_ref(ctx).session(terminal_view.id()),
        restore_payload,
        app_run_id: ctx
            .has_singleton_model::<SessionMemoryModel>()
            .then(|| SessionMemoryModel::as_ref(ctx).current_run_id().to_string()),
        now: now_unix_seconds(),
    })
}
```

3. `fn session_memory_record_id(&self) -> String` body becomes `record_id_for_pane_uuid(&self.uuid)`.
4. Replace `upsert_session_memory_record` with:

```rust
    fn upsert_session_memory_record(&self, ctx: &AppContext) {
        if !AppExecutionMode::as_ref(ctx).can_save_session() {
            return;
        }

        let Some(sender) = &self.model_event_sender else {
            return;
        };

        let terminal_view = self.terminal_view(ctx);
        let record = session_memory_record_for_pane(&self.uuid, &terminal_view, ctx);
        if let Err(err) = sender.send(ModelEvent::UpsertSessionMemoryRecord { record }) {
            log::error!(
                "Error sending session memory upsert event for terminal id {} {:?}",
                terminal_view.id(),
                err
            );
        }
    }

    pub(in crate::pane_group) fn mark_session_memory_agent_ended(
        &self,
        block: &SerializedBlock,
        ctx: &AppContext,
    ) {
        if !AppExecutionMode::as_ref(ctx).can_save_session()
            || ctx.windows().stage() == ApplicationStage::Terminating
        {
            return;
        }
        let Some(sender) = &self.model_event_sender else {
            return;
        };
        let Some(started_at) = block_timestamp_seconds(block.start_ts.as_ref()) else {
            return;
        };
        let completed_at =
            block_timestamp_seconds(block.completed_ts.as_ref()).unwrap_or_else(now_unix_seconds);

        let model_event = ModelEvent::MarkSessionMemoryAgentEnded {
            id: self.session_memory_record_id(),
            started_at,
            completed_at,
        };
        if let Err(err) = sender.send(model_event) {
            log::error!("Error sending session memory agent end event: {err:?}");
        }
    }
```

5. In `snapshot`, replace

```rust
            self.upsert_session_memory_record(
                &snapshot,
                terminal_last_command(view.session_command_context(app)),
                app,
            );
```

with `self.upsert_session_memory_record(app);`.

6. In `attach`, replace the `CLIAgentSessionsModel` subscription closure body with:

```rust
                move |_group, _sessions_model, event, ctx| {
                    if event.terminal_view_id() != terminal_view_id
                        || matches!(event, CLIAgentSessionsModelEvent::Ended { .. })
                    {
                        return;
                    }

                    let record = session_memory_record_for_pane(&uuid, &terminal_view, ctx);
                    if let Err(err) =
                        model_event_sender.send(ModelEvent::UpsertSessionMemoryRecord { record })
                    {
                        log::error!(
                            "Error sending CLI agent session memory upsert event for terminal id {} {:?}",
                            terminal_view_id,
                            err
                        );
                    }
                },
```

7. In `handle_terminal_view_event`, inside `Event::BlockCompleted { block, is_local } => match group.terminal_session_by_id(pane_id) { Some(pane) => {`, add as the first statement:

```rust
                        pane.mark_session_memory_agent_ended(block, ctx);
```

8. Imports: add

```rust
use crate::session_memory::recording::{
    PaneRecordInput, block_timestamp_seconds, pane_session_memory_record,
    record_id_for_pane_uuid,
};
use crate::terminal::cli_agent_sessions::CLIAgentSessionsModelEvent;
use crate::terminal::model::block::SerializedBlock;
use warpui::windowing::state::ApplicationStage;
```

Remove the imports this orphans: `AgentPermissionMode, SessionMemoryKind, SessionMemorySource, SessionMemoryStatus` from `crate::persistence`, `terminal_agent_command`, `CLIAgentSession, CLIAgentSessionStatus`, the base64 imports and `CLIAgent`. Remove each one only if `cargo check` reports it unused; keep any the rest of the file still needs.

In `app/src/terminal/view.rs` (~12478-12496), delete the whole block from the comment `// The agent process exited while the pane stays open: the` through the closing `}` of `if let Some(native_session_id) = ended_native_session_id ... { ... }`. Keep the following `CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions_model, ctx| { sessions_model.remove_session(self.view_id, ctx); });`.

In `app/src/session_memory/model.rs`, delete `mark_agent_session_ended`, `mark_agent_session_ended_and_notify` and `mark_agent_session_ended_for_native_session_and_notify`.

- [ ] **Step 4: Verify**

Run: `cargo check -p warp`, then `cargo test -p warp --lib session_memory`, then `cargo clippy -p warp --all-targets --tests -- -D warnings`.
Expected: clean check and clippy. All session_memory tests pass.

- [ ] **Step 5: Commit**

```bash
git add app/src/pane_group/pane/terminal_pane.rs app/src/terminal/view.rs app/src/session_memory/model.rs app/src/session_memory/model_tests.rs
git commit -m "fix(session-memory): record agents from the running block and persist exits"
```

---

## Task 5: Startup candidates from the previous run only

**Files:**
- Modify: `app/src/session_memory/model.rs` (whole file; full content below)
- Modify: `app/src/session_memory/types.rs` (delete `normalize_terminal_agent_command`, ~185-198)
- Modify: `app/src/session_memory/model_tests.rs`
- Modify: `app/src/persistence/mod.rs` (`enum ModelEvent`)
- Modify: `app/src/persistence/sqlite.rs` (`handle_model_event`, new fn)
- Modify: `app/src/persistence/sqlite_tests.rs`

**Interfaces:**
- Produces:
  - `SessionMemoryModel::startup_restore_candidates(&self) -> Vec<SessionMemoryRecord>`
  - `SessionMemoryModel::previous_run_native_session_ids(&self) -> HashSet<String>`
  - `SessionMemoryModel::upsert(&mut self, record: SessionMemoryRecord)` (now returns `()`)
  - `SessionMemoryModelEvent::MarkRecordsOffered { ids: Vec<String>, app_run_id: String, offered_run_id: String }`
  - `ModelEvent::MarkSessionMemoryRecordsOffered { ids: Vec<String>, app_run_id: String, offered_run_id: String }`
- Removes: `startup_auto_restore_records`, `should_auto_restore_on_startup`, `is_recent_agent_startup_restore_candidate`, `RECENT_AGENT_STARTUP_RESTORE_SECONDS`, `dedupe_native_session_duplicates`, `dedupe_native_session_duplicates_of`, `SessionMemoryRecord::normalize_terminal_agent_command`, and the model's `now_unix_seconds`.
- Consumed by: Task 9.

- [ ] **Step 1: Update tests (old behavior out, new behavior in)**

Remove from `model_tests.rs`, each for the reason given:
- `startup_reclassifies_terminal_hosted_claude_command` and `startup_reclassifies_terminal_hosted_codex_command`. Reason: agent kind now comes from the running command when the record is written (Task 1). Reclassifying from `last_command` of restored blocks is the defect.
- `upsert_removes_older_records_for_same_native_session` and `load_dedupes_records_sharing_native_session_keeping_newest`. Reason: the spec forbids deleting other panes' records. Dedupe now happens only in candidate selection.
- `startup_auto_restore_records_includes_recent_resumable_agent_from_older_run`. Reason: the 30-minute rule is removed.
- `startup_auto_restore_records_only_returns_previous_unoffered_run`, `startup_auto_restore_records_excludes_completed_agent_sessions` and `startup_auto_restore_records_includes_resumable_sessions_from_clean_previous_run`. Reason: the API is replaced by `startup_restore_candidates`, and startup tmux auto-run is removed (spec: plain terminals are not re-run). They are replaced by the tests below.

Replace `mark_startup_recovery_offered_persists_one_shot_marker`'s `match receiver.recv().unwrap() { ... }` with:

```rust
    match receiver.recv().unwrap() {
        ModelEvent::MarkSessionMemoryRecordsOffered {
            ids,
            app_run_id,
            offered_run_id,
        } => {
            assert_eq!(ids, vec!["previous-run-session".to_string()]);
            assert_eq!(app_run_id, "previous-run");
            assert_eq!(offered_run_id, "current-run");
        }
        event => panic!("expected session memory offered event, got {event:?}"),
    }
```

Add to `model_tests.rs`:

```rust
#[test]
fn startup_restore_candidates_only_include_previous_run() {
    let model = model_with_previous_run(vec![
        agent_record("previous-run-agent", "previous-run"),
        agent_record("older-run-agent", "older-run"),
        agent_record("current-run-agent", "current-run"),
    ]);

    assert_eq!(candidate_ids(&model), vec!["previous-run-agent"]);
}

#[test]
fn startup_restore_candidates_exclude_recent_agents_from_older_runs() {
    let mut recent_older = agent_record("recent-older-run-agent", "older-run");
    recent_older.last_seen_at = current_unix_seconds();
    let model = model_with_previous_run(vec![recent_older]);

    assert!(model.startup_restore_candidates().is_empty());
}

#[test]
fn startup_restore_candidates_include_idle_agents_and_skip_terminals() {
    let mut idle = agent_record("idle-claude", "previous-run");
    idle.status = SessionMemoryStatus::Success;
    let mut blocked = agent_record("blocked-codex", "previous-run");
    blocked.source = SessionMemorySource::Codex;
    blocked.status = SessionMemoryStatus::Blocked;
    let mut tmux = agent_record("tmux-terminal", "previous-run");
    tmux.source = SessionMemorySource::WarpTerminal;
    tmux.kind = SessionMemoryKind::Terminal;
    tmux.last_command = Some("tmux attach -t work".to_string());
    let mut restored_block = agent_record("restored-claude-block", "previous-run");
    restored_block.source = SessionMemorySource::WarpTerminal;
    restored_block.kind = SessionMemoryKind::Terminal;
    restored_block.last_command = Some("claude --resume abc".to_string());
    let model = model_with_previous_run(vec![idle, blocked, tmux, restored_block]);

    assert_eq!(candidate_ids(&model), vec!["idle-claude", "blocked-codex"]);
}

#[test]
fn startup_restore_candidates_exclude_ended_closed_and_offered_agents() {
    let mut ended = agent_record("ended-agent", "previous-run");
    ended.completed_at = Some(150);
    let mut closed = agent_record("closed-agent", "previous-run");
    closed.closed_intentionally_at = Some(150);
    let mut offered = agent_record("offered-agent", "previous-run");
    offered.recovery_offered_run_id = Some("current-run".to_string());
    let open = agent_record("open-agent", "previous-run");
    let model = model_with_previous_run(vec![ended, closed, offered, open]);

    assert_eq!(candidate_ids(&model), vec!["open-agent"]);
}

#[test]
fn startup_restore_candidates_keep_newest_record_per_native_session() {
    let mut old = agent_record("old-pane", "previous-run");
    old.native_session_id = Some("chat-1".to_string());
    old.last_seen_at = 100;
    let mut new = agent_record("new-pane", "previous-run");
    new.native_session_id = Some("chat-1".to_string());
    new.last_seen_at = 200;
    let mut other = agent_record("other-pane", "previous-run");
    other.native_session_id = Some("chat-2".to_string());
    let model = model_with_previous_run(vec![old, new, other]);

    assert_eq!(candidate_ids(&model), vec!["new-pane", "other-pane"]);
    assert_eq!(model.records().len(), 3);
}

#[test]
fn startup_restore_candidates_without_previous_run_are_empty() {
    let model = SessionMemoryModel::new_with_run_state(
        vec![agent_record("agent", "previous-run")],
        None,
        SessionMemoryRunState::with_previous_run("current-run", None, None),
    );

    assert!(model.startup_restore_candidates().is_empty());
}

#[test]
fn previous_run_native_session_ids_collects_only_previous_run_ids() {
    let mut previous = agent_record("previous", "previous-run");
    previous.native_session_id = Some("previous-id".to_string());
    let mut older = agent_record("older", "older-run");
    older.native_session_id = Some("older-id".to_string());
    let model = model_with_previous_run(vec![previous, older]);

    let ids = model.previous_run_native_session_ids();

    assert!(ids.contains("previous-id"));
    assert!(!ids.contains("older-id"));
}

fn agent_record(id: &str, app_run_id: &str) -> SessionMemoryRecord {
    let mut record = test_record(id);
    record.source = SessionMemorySource::ClaudeCode;
    record.kind = SessionMemoryKind::AgentChat;
    record.status = SessionMemoryStatus::Live;
    record.app_run_id = Some(app_run_id.to_string());
    record
}

fn model_with_previous_run(records: Vec<SessionMemoryRecord>) -> SessionMemoryModel {
    SessionMemoryModel::new_with_run_state(
        records,
        None,
        SessionMemoryRunState::with_previous_run(
            "current-run",
            Some("previous-run".to_string()),
            None,
        ),
    )
}

fn candidate_ids(model: &SessionMemoryModel) -> Vec<String> {
    model
        .startup_restore_candidates()
        .into_iter()
        .map(|record| record.id)
        .collect()
}
```

Append to `sqlite_tests.rs`:

```rust
#[test]
fn mark_records_offered_skips_rows_rewritten_by_current_run() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut conn = setup_database(&tempdir.path().join("warp.sqlite"))
        .expect("database should initialize");
    let mut previous = session_memory_agent_record(Some(100));
    previous.id = "previous-pane".to_string();
    previous.app_run_id = Some("previous-run".to_string());
    let mut rewritten = session_memory_agent_record(Some(300));
    rewritten.id = "rewritten-pane".to_string();
    rewritten.app_run_id = Some("current-run".to_string());
    upsert_session_memory(&mut conn, previous);
    upsert_session_memory(&mut conn, rewritten);

    handle_model_event(
        ModelEvent::MarkSessionMemoryRecordsOffered {
            ids: vec!["previous-pane".to_string(), "rewritten-pane".to_string()],
            app_run_id: "previous-run".to_string(),
            offered_run_id: "current-run".to_string(),
        },
        &mut conn,
    )
    .expect("offered marker should be written");

    let records = read_sqlite_data(&mut conn, None, PersistedDataScope::Full)
        .expect("app state should load")
        .session_memory_records;
    let offered = |id: &str| {
        records
            .iter()
            .find(|record| record.id == id)
            .expect("record should exist")
            .recovery_offered_run_id
            .clone()
    };
    assert_eq!(offered("previous-pane").as_deref(), Some("current-run"));
    assert_eq!(offered("rewritten-pane"), None);
}
```

- [ ] **Step 2: Run the tests (expect failure)**

Run: `cargo test -p warp --lib session_memory`
Expected: compile errors `no method named startup_restore_candidates` and `no variant named MarkSessionMemoryRecordsOffered`.

- [ ] **Step 3: Implement**

`app/src/persistence/mod.rs`, in `enum ModelEvent` after `MarkSessionMemoryAgentEnded { .. },`:

```rust
    MarkSessionMemoryRecordsOffered {
        ids: Vec<String>,
        app_run_id: String,
        offered_run_id: String,
    },
```

`app/src/persistence/sqlite.rs`, `handle_model_event` arm:

```rust
        ModelEvent::MarkSessionMemoryRecordsOffered {
            ids,
            app_run_id,
            offered_run_id,
        } => mark_session_memory_records_offered(connection, &ids, &app_run_id, &offered_run_id)
            .context("error marking session memory records offered"),
```

and the fn after `mark_session_memory_agent_ended`:

```rust
fn mark_session_memory_records_offered(
    conn: &mut SqliteConnection,
    record_ids: &[String],
    app_run_id: &str,
    offered_run_id: &str,
) -> Result<()> {
    diesel::update(
        schema::session_memory_records::dsl::session_memory_records
            .filter(schema::session_memory_records::dsl::id.eq_any(record_ids))
            .filter(schema::session_memory_records::dsl::app_run_id.eq(app_run_id)),
    )
    .set(schema::session_memory_records::dsl::recovery_offered_run_id.eq(offered_run_id))
    .execute(conn)?;
    Ok(())
}
```

`app/src/session_memory/types.rs`: delete `pub fn normalize_terminal_agent_command(&mut self) { ... }`.

Replace `app/src/session_memory/model.rs` with:

```rust
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;

use warpui::{Entity, ModelContext, SingletonEntity};

use crate::persistence::{self, ModelEvent};

use super::types::{SessionMemoryRecord, SessionMemoryRunState, SessionMemoryStatus};

pub type SessionMemoryEventSink = Arc<dyn Fn(SessionMemoryModelEvent) + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMemoryModelEvent {
    UpsertRecord {
        record: SessionMemoryRecord,
    },
    DeleteRecord {
        id: String,
    },
    MarkRecordsOffered {
        ids: Vec<String>,
        app_run_id: String,
        offered_run_id: String,
    },
}

pub struct SessionMemoryModel {
    records: Vec<SessionMemoryRecord>,
    event_sink: Option<SessionMemoryEventSink>,
    run_state: SessionMemoryRunState,
}

impl SessionMemoryModel {
    pub fn new(
        records: Vec<SessionMemoryRecord>,
        event_sink: Option<SessionMemoryEventSink>,
    ) -> Self {
        Self::new_with_run_state(records, event_sink, SessionMemoryRunState::test_default())
    }

    pub fn new_with_run_state(
        mut records: Vec<SessionMemoryRecord>,
        event_sink: Option<SessionMemoryEventSink>,
        run_state: SessionMemoryRunState,
    ) -> Self {
        for record in &mut records {
            record.status = record
                .status
                .classify_startup(record.closed_intentionally_at);
        }

        Self {
            records,
            event_sink,
            run_state,
        }
    }

    pub fn from_persisted_records(
        records: Vec<persistence::SessionMemoryRecord>,
        event_sink: Option<SessionMemoryEventSink>,
    ) -> Self {
        Self::from_persisted_records_with_run_state(
            records,
            event_sink,
            SessionMemoryRunState::test_default(),
        )
    }

    pub fn from_persisted_records_with_run_state(
        records: Vec<persistence::SessionMemoryRecord>,
        event_sink: Option<SessionMemoryEventSink>,
        run_state: SessionMemoryRunState,
    ) -> Self {
        Self::new_with_run_state(
            records.into_iter().map(SessionMemoryRecord::from).collect(),
            event_sink,
            run_state,
        )
    }

    pub fn persistence_event_sink(
        sender: Option<SyncSender<ModelEvent>>,
    ) -> Option<SessionMemoryEventSink> {
        sender.map(|sender| {
            Arc::new(move |event| {
                let model_event = match event {
                    SessionMemoryModelEvent::UpsertRecord { record } => {
                        ModelEvent::UpsertSessionMemoryRecord {
                            record: record.into(),
                        }
                    }
                    SessionMemoryModelEvent::DeleteRecord { id } => {
                        ModelEvent::DeleteSessionMemoryRecord { id }
                    }
                    SessionMemoryModelEvent::MarkRecordsOffered {
                        ids,
                        app_run_id,
                        offered_run_id,
                    } => ModelEvent::MarkSessionMemoryRecordsOffered {
                        ids,
                        app_run_id,
                        offered_run_id,
                    },
                };

                if let Err(err) = sender.send(model_event) {
                    log::error!("Error sending session memory model event to persistence: {err:?}");
                }
            }) as SessionMemoryEventSink
        })
    }

    pub fn records(&self) -> &[SessionMemoryRecord] {
        &self.records
    }

    pub fn current_run_id(&self) -> &str {
        &self.run_state.current_run_id
    }

    pub fn interrupted_count(&self) -> usize {
        self.records
            .iter()
            .filter(|record| record.status == SessionMemoryStatus::Interrupted)
            .count()
    }

    pub fn interrupted_records(&self) -> Vec<SessionMemoryRecord> {
        self.records
            .iter()
            .filter(|record| record.status == SessionMemoryStatus::Interrupted)
            .cloned()
            .collect()
    }

    pub fn startup_restore_candidates(&self) -> Vec<SessionMemoryRecord> {
        let Some(previous_run_id) = self.run_state.previous_run_id.as_deref() else {
            return Vec::new();
        };

        let mut candidates: Vec<SessionMemoryRecord> = Vec::new();
        for record in self.records.iter().filter(|record| {
            record.is_agent()
                && record.app_run_id.as_deref() == Some(previous_run_id)
                && record.completed_at.is_none()
                && record.closed_intentionally_at.is_none()
                && record.recovery_offered_run_id.is_none()
        }) {
            let duplicate = record.native_session_id.as_ref().and_then(|native_session_id| {
                candidates.iter().position(|candidate| {
                    candidate.native_session_id.as_ref() == Some(native_session_id)
                })
            });
            match duplicate {
                Some(index) if candidates[index].last_seen_at >= record.last_seen_at => {}
                Some(index) => candidates[index] = record.clone(),
                None => candidates.push(record.clone()),
            }
        }
        candidates
    }

    pub fn previous_run_native_session_ids(&self) -> HashSet<String> {
        let Some(previous_run_id) = self.run_state.previous_run_id.as_deref() else {
            return HashSet::new();
        };
        self.records
            .iter()
            .filter(|record| record.app_run_id.as_deref() == Some(previous_run_id))
            .filter_map(|record| record.native_session_id.clone())
            .collect()
    }

    pub fn filtered_records(&self, query: &str) -> Vec<SessionMemoryRecord> {
        self.records
            .iter()
            .filter(|record| record.matches_query(query))
            .cloned()
            .collect()
    }

    pub fn upsert(&mut self, record: SessionMemoryRecord) {
        let mut record = record;
        if record.app_run_id.is_none() {
            record.app_run_id = Some(self.run_state.current_run_id.clone());
        }

        if let Some(existing) = self
            .records
            .iter_mut()
            .find(|existing| existing.id == record.id)
        {
            *existing = record.clone();
        } else {
            self.records.push(record.clone());
        }

        if let Some(event_sink) = &self.event_sink {
            event_sink(SessionMemoryModelEvent::UpsertRecord { record });
        }
    }

    pub fn upsert_and_notify(&mut self, record: SessionMemoryRecord, ctx: &mut ModelContext<Self>) {
        self.upsert(record.clone());
        ctx.emit(SessionMemoryModelEvent::UpsertRecord { record });
    }

    pub fn delete(&mut self, id: &str) {
        self.records.retain(|record| record.id != id);

        if let Some(event_sink) = &self.event_sink {
            event_sink(SessionMemoryModelEvent::DeleteRecord { id: id.to_string() });
        }
    }

    pub fn delete_and_notify(&mut self, id: &str, ctx: &mut ModelContext<Self>) {
        self.delete(id);
        ctx.emit(SessionMemoryModelEvent::DeleteRecord { id: id.to_string() });
    }

    /// True when a layout-restored tab should be skipped because every
    /// terminal pane in it was already closed intentionally by the user. The
    /// window snapshot can be stale after a crash or force-kill, while close
    /// markers are written immediately — trust the markers.
    pub fn should_suppress_restored_tab(&self, terminal_pane_uuids: &[Vec<u8>]) -> bool {
        if terminal_pane_uuids.is_empty() {
            return false;
        }
        terminal_pane_uuids.iter().all(|uuid| {
            self.records.iter().any(|record| {
                record.terminal_pane_uuid.as_deref() == Some(uuid.as_slice())
                    && record.closed_intentionally_at.is_some()
            })
        })
    }

    pub fn mark_startup_recovery_offered(&mut self, ids: &[String]) {
        let Some(previous_run_id) = self.run_state.previous_run_id.clone() else {
            return;
        };
        let offered_run_id = self.run_state.current_run_id.clone();
        for record in &mut self.records {
            if ids.iter().any(|id| id == &record.id)
                && record.app_run_id.as_deref() == Some(previous_run_id.as_str())
            {
                record.recovery_offered_run_id = Some(offered_run_id.clone());
            }
        }

        if let Some(event_sink) = &self.event_sink {
            event_sink(SessionMemoryModelEvent::MarkRecordsOffered {
                ids: ids.to_vec(),
                app_run_id: previous_run_id,
                offered_run_id,
            });
        }
    }

    pub fn mark_startup_recovery_offered_and_notify(
        &mut self,
        ids: &[String],
        ctx: &mut ModelContext<Self>,
    ) {
        self.mark_startup_recovery_offered(ids);
        for record in self
            .records
            .iter()
            .filter(|record| ids.iter().any(|id| id == &record.id))
            .cloned()
        {
            ctx.emit(SessionMemoryModelEvent::UpsertRecord { record });
        }
    }
}

impl Entity for SessionMemoryModel {
    type Event = SessionMemoryModelEvent;
}

impl SingletonEntity for SessionMemoryModel {}
```

(`should_suppress_restored_tab`'s doc comment is pre-existing and stays unchanged.)

`app/src/workspace/view.rs` still calls `startup_auto_restore_records` and `upsert_and_notify` from `auto_restore_startup_session_memory` / `enrich_*` until Task 9. To keep this task compiling, make `auto_restore_startup_session_memory` call `startup_restore_candidates()` in place of `startup_auto_restore_records()`. That one-line bridge is removed with the whole function in Task 9.

- [ ] **Step 4: Run the tests (expect pass)**

Run: `cargo test -p warp --lib session_memory`, then `cargo check -p warp`.
Expected: all model and sqlite session-memory tests pass. The check is clean.

- [ ] **Step 5: Commit**

```bash
git add app/src/session_memory/model.rs app/src/session_memory/model_tests.rs app/src/session_memory/types.rs app/src/persistence/mod.rs app/src/persistence/sqlite.rs app/src/persistence/sqlite_tests.rs app/src/workspace/view.rs
git commit -m "fix(session-memory): pick startup candidates from the previous run only"
```

---

## Task 6: Startup resume plan, session-file matching and window-aware targeting

**Files:**
- Modify: `app/src/session_memory/restore.rs` (append new items; old startup API stays until Task 9)
- Modify: `app/src/session_memory/restore_tests.rs` (append)

**Interfaces:**
- Consumes: `CLIAgent::{resume_command_preserving_permission, dangerous_flag}`, `SessionMemoryRecord::is_agent` (not required), `SessionMemorySource`.
- Produces:
  - `pub const SESSION_FILE_START_TOLERANCE_SECONDS: i64 = 2;`
  - `pub struct AgentSessionFile { pub session_id: String, pub created_at: i64 }`
  - `pub fn match_session_file(started_at: i64, files: &[AgentSessionFile], claimed: &HashSet<String>) -> Option<String>`
  - `pub fn resolve_missing_session_ids(candidates: &mut [SessionMemoryRecord], claimed: HashSet<String>, session_files: impl FnMut(SessionMemorySource, &Path) -> Vec<AgentSessionFile>)`
  - `pub fn startup_agent_restore_plan(record: &SessionMemoryRecord) -> Result<RestorePlan, RestoreError>`
  - `pub enum StartupRestoreTarget<W> { ExistingPane { window: W, terminal_pane_uuid: Vec<u8>, plan: RestorePlan }, NewTab { plan: RestorePlan }, Skip(StartupRestoreSkip) }`
  - `pub enum StartupRestoreSkip { PaneGone, DuplicateContinue, Invalid(RestoreError) }`
  - `pub fn plan_startup_restore<W: Clone>(candidates: &[SessionMemoryRecord], restored_panes: &[(W, Vec<u8>)], layout_restore_enabled: bool) -> Vec<(String, StartupRestoreTarget<W>)>`
- Consumed by: Task 9.

- [ ] **Step 1: Write the failing tests**

Append to `app/src/session_memory/restore_tests.rs`. Add `AgentSessionFile, StartupRestoreSkip, StartupRestoreTarget, match_session_file, plan_startup_restore, resolve_missing_session_ids, startup_agent_restore_plan` to the `use super::restore::{...}` list and `use std::collections::HashSet;` at the top.

```rust
#[test]
fn startup_plan_resumes_claude_with_id_and_dangerous_flag() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let record = startup_agent(tempdir.path().to_path_buf(), SessionMemorySource::ClaudeCode, Some("abc"), AgentPermissionMode::Dangerous);

    let plan = startup_agent_restore_plan(&record).expect("plan should be built");

    assert_eq!(plan.command(), Some("claude --resume abc --dangerously-skip-permissions"));
    assert_eq!(plan.cwd(), Some(tempdir.path()));
}

#[test]
fn startup_plan_continues_claude_without_id() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let record = startup_agent(tempdir.path().to_path_buf(), SessionMemorySource::ClaudeCode, None, AgentPermissionMode::Dangerous);

    let plan = startup_agent_restore_plan(&record).expect("plan should be built");

    assert_eq!(plan.command(), Some("claude --continue --dangerously-skip-permissions"));
}

#[test]
fn startup_plan_resumes_last_codex_without_id() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let record = startup_agent(tempdir.path().to_path_buf(), SessionMemorySource::Codex, None, AgentPermissionMode::Normal);

    let plan = startup_agent_restore_plan(&record).expect("plan should be built");

    assert_eq!(plan.command(), Some("codex resume --last"));
}

#[test]
fn startup_plan_resumes_codex_with_id() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let record = startup_agent(tempdir.path().to_path_buf(), SessionMemorySource::Codex, Some("abc"), AgentPermissionMode::Unknown);

    let plan = startup_agent_restore_plan(&record).expect("plan should be built");

    assert_eq!(plan.command(), Some("codex resume abc"));
}

#[test]
fn startup_plan_rejects_missing_cwd() {
    let missing = PathBuf::from("/tmp/session-memory-missing-cwd-for-startup-plan");
    let record = startup_agent(missing.clone(), SessionMemorySource::ClaudeCode, Some("abc"), AgentPermissionMode::Normal);

    assert_eq!(
        startup_agent_restore_plan(&record),
        Err(RestoreError::MissingWorkingDirectory(missing))
    );
}

#[test]
fn startup_restore_targets_panes_across_two_windows() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut first = startup_agent(tempdir.path().to_path_buf(), SessionMemorySource::ClaudeCode, Some("one"), AgentPermissionMode::Normal);
    first.id = "first".to_string();
    first.terminal_pane_uuid = Some(vec![1]);
    let mut second = startup_agent(tempdir.path().to_path_buf(), SessionMemorySource::Codex, Some("two"), AgentPermissionMode::Normal);
    second.id = "second".to_string();
    second.terminal_pane_uuid = Some(vec![2]);
    let restored = vec![("window-a", vec![1]), ("window-b", vec![9]), ("window-b", vec![2])];

    let targets = plan_startup_restore(&[first, second], &restored, true);

    let target_of = |id: &str| {
        targets
            .iter()
            .find(|(record_id, _)| record_id == id)
            .map(|(_, target)| target.clone())
            .expect("target should exist")
    };
    match target_of("first") {
        StartupRestoreTarget::ExistingPane { window, terminal_pane_uuid, plan } => {
            assert_eq!(window, "window-a");
            assert_eq!(terminal_pane_uuid, vec![1]);
            assert_eq!(plan.command(), Some("claude --resume one"));
        }
        other => panic!("expected existing pane, got {other:?}"),
    }
    match target_of("second") {
        StartupRestoreTarget::ExistingPane { window, terminal_pane_uuid, plan } => {
            assert_eq!(window, "window-b");
            assert_eq!(terminal_pane_uuid, vec![2]);
            assert_eq!(plan.command(), Some("codex resume two"));
        }
        other => panic!("expected existing pane, got {other:?}"),
    }
}

#[test]
fn startup_restore_skips_missing_pane_when_layout_restore_is_on() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = startup_agent(tempdir.path().to_path_buf(), SessionMemorySource::ClaudeCode, Some("abc"), AgentPermissionMode::Normal);
    record.terminal_pane_uuid = Some(vec![7]);

    let targets = plan_startup_restore(&[record], &[("window-a", vec![1])], true);

    assert_eq!(targets[0].1, StartupRestoreTarget::Skip(StartupRestoreSkip::PaneGone));
}

#[test]
fn startup_restore_opens_new_tab_when_layout_restore_is_off() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = startup_agent(tempdir.path().to_path_buf(), SessionMemorySource::ClaudeCode, Some("abc"), AgentPermissionMode::Normal);
    record.terminal_pane_uuid = Some(vec![7]);

    let targets = plan_startup_restore::<&str>(&[record], &[], false);

    match &targets[0].1 {
        StartupRestoreTarget::NewTab { plan } => {
            assert_eq!(plan.command(), Some("claude --resume abc"));
            assert_eq!(plan.cwd(), Some(tempdir.path()));
        }
        other => panic!("expected new tab, got {other:?}"),
    }
}

#[test]
fn startup_restore_continues_only_newest_idless_candidate_per_cwd() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut older = startup_agent(tempdir.path().to_path_buf(), SessionMemorySource::ClaudeCode, None, AgentPermissionMode::Normal);
    older.id = "older".to_string();
    older.terminal_pane_uuid = Some(vec![1]);
    older.last_seen_at = 100;
    let mut newer = older.clone();
    newer.id = "newer".to_string();
    newer.terminal_pane_uuid = Some(vec![2]);
    newer.last_seen_at = 200;
    let restored = vec![("window-a", vec![1]), ("window-a", vec![2])];

    let targets = plan_startup_restore(&[older, newer], &restored, true);

    let target_of = |id: &str| {
        targets
            .iter()
            .find(|(record_id, _)| record_id == id)
            .map(|(_, target)| target.clone())
            .expect("target should exist")
    };
    assert!(matches!(target_of("newer"), StartupRestoreTarget::ExistingPane { .. }));
    assert_eq!(
        target_of("older"),
        StartupRestoreTarget::Skip(StartupRestoreSkip::DuplicateContinue)
    );
}

#[test]
fn match_session_file_picks_earliest_unclaimed_file_after_start() {
    let files = vec![
        session_file("before-start", 90),
        session_file("claimed", 101),
        session_file("second", 120),
        session_file("first", 105),
    ];
    let claimed = HashSet::from(["claimed".to_string()]);

    assert_eq!(match_session_file(100, &files, &claimed).as_deref(), Some("first"));
}

#[test]
fn match_session_file_ignores_files_created_before_start() {
    let files = vec![session_file("old", 50)];

    assert_eq!(match_session_file(100, &files, &HashSet::new()), None);
}

#[test]
fn match_session_file_accepts_tolerance_before_start() {
    let files = vec![session_file("just-before", 99)];

    assert_eq!(
        match_session_file(100, &files, &HashSet::new()).as_deref(),
        Some("just-before")
    );
}

#[test]
fn resolve_missing_session_ids_does_not_assign_one_file_to_two_panes() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut first = startup_agent(tempdir.path().to_path_buf(), SessionMemorySource::ClaudeCode, None, AgentPermissionMode::Normal);
    first.id = "first".to_string();
    first.started_at = Some(100);
    let mut second = first.clone();
    second.id = "second".to_string();
    second.started_at = Some(110);
    let mut with_id = first.clone();
    with_id.id = "with-id".to_string();
    with_id.native_session_id = Some("taken".to_string());
    let mut candidates = vec![second, first, with_id];

    resolve_missing_session_ids(&mut candidates, HashSet::new(), |_, _| {
        vec![
            session_file("taken", 100),
            session_file("file-a", 101),
            session_file("file-b", 111),
        ]
    });

    let id_of = |id: &str| {
        candidates
            .iter()
            .find(|candidate| candidate.id == id)
            .and_then(|candidate| candidate.native_session_id.clone())
    };
    assert_eq!(id_of("first").as_deref(), Some("file-a"));
    assert_eq!(id_of("second").as_deref(), Some("file-b"));
    assert_eq!(id_of("with-id").as_deref(), Some("taken"));
}

fn startup_agent(
    cwd: PathBuf,
    source: SessionMemorySource,
    session_id: Option<&str>,
    permission_mode: AgentPermissionMode,
) -> SessionMemoryRecord {
    let mut record = codex_record(cwd, "unused");
    record.source = source;
    record.native_session_id = session_id.map(str::to_owned);
    record.permission_mode = permission_mode;
    record.status = SessionMemoryStatus::Interrupted;
    record
}

fn session_file(session_id: &str, created_at: i64) -> AgentSessionFile {
    AgentSessionFile {
        session_id: session_id.to_string(),
        created_at,
    }
}
```

Run `cargo fmt` after pasting. The long single-line `startup_agent(...)` calls are reflowed.

- [ ] **Step 2: Run the tests (expect failure)**

Run: `cargo test -p warp --lib session_memory::restore`
Expected: compile errors for the unresolved imports `plan_startup_restore`, `startup_agent_restore_plan`, and so on.

- [ ] **Step 3: Implement**

Add to the top of `app/src/session_memory/restore.rs`: `use std::collections::HashSet;`. Append:

```rust
pub const SESSION_FILE_START_TOLERANCE_SECONDS: i64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionFile {
    pub session_id: String,
    pub created_at: i64,
}

pub fn match_session_file(
    started_at: i64,
    files: &[AgentSessionFile],
    claimed: &HashSet<String>,
) -> Option<String> {
    files
        .iter()
        .filter(|file| {
            file.created_at + SESSION_FILE_START_TOLERANCE_SECONDS >= started_at
                && !claimed.contains(&file.session_id)
        })
        .min_by_key(|file| file.created_at)
        .map(|file| file.session_id.clone())
}

pub fn resolve_missing_session_ids(
    candidates: &mut [SessionMemoryRecord],
    mut claimed: HashSet<String>,
    mut session_files: impl FnMut(SessionMemorySource, &Path) -> Vec<AgentSessionFile>,
) {
    claimed.extend(
        candidates
            .iter()
            .filter_map(|candidate| candidate.native_session_id.clone()),
    );
    let mut order = (0..candidates.len()).collect::<Vec<_>>();
    order.sort_by_key(|&index| candidates[index].started_at);

    for index in order {
        let candidate = &candidates[index];
        if candidate.native_session_id.is_some() {
            continue;
        }
        let (Some(started_at), Some(cwd)) = (candidate.started_at, candidate.cwd.clone()) else {
            continue;
        };
        let files = session_files(candidate.source, &cwd);
        if let Some(session_id) = match_session_file(started_at, &files, &claimed) {
            claimed.insert(session_id.clone());
            candidates[index].native_session_id = Some(session_id);
        }
    }
}

pub fn startup_agent_restore_plan(record: &SessionMemoryRecord) -> Result<RestorePlan, RestoreError> {
    let (agent, continue_command) = match record.source {
        SessionMemorySource::ClaudeCode => (CLIAgent::Claude, "claude --continue"),
        SessionMemorySource::Codex => (CLIAgent::Codex, "codex resume --last"),
        SessionMemorySource::WarpTerminal => return Err(RestoreError::UnsupportedSource),
    };
    let cwd = record
        .cwd
        .clone()
        .ok_or_else(|| RestoreError::MissingWorkingDirectory(PathBuf::new()))?;
    if !cwd.exists() {
        return Err(RestoreError::MissingWorkingDirectory(cwd));
    }

    let command = match record.native_session_id.as_deref() {
        Some(session_id) => {
            agent.resume_command_preserving_permission(session_id, record.permission_mode)
        }
        None => match (record.permission_mode, agent.dangerous_flag()) {
            (AgentPermissionMode::Dangerous, Some(flag)) => format!("{continue_command} {flag}"),
            _ => continue_command.to_owned(),
        },
    };

    Ok(RestorePlan::Agent {
        agent,
        cwd,
        command,
        permission_mode: record.permission_mode,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupRestoreSkip {
    PaneGone,
    DuplicateContinue,
    Invalid(RestoreError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupRestoreTarget<W> {
    ExistingPane {
        window: W,
        terminal_pane_uuid: Vec<u8>,
        plan: RestorePlan,
    },
    NewTab {
        plan: RestorePlan,
    },
    Skip(StartupRestoreSkip),
}

pub fn plan_startup_restore<W: Clone>(
    candidates: &[SessionMemoryRecord],
    restored_panes: &[(W, Vec<u8>)],
    layout_restore_enabled: bool,
) -> Vec<(String, StartupRestoreTarget<W>)> {
    let mut ordered = candidates.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| b.last_seen_at.cmp(&a.last_seen_at));

    let mut continued: Vec<(SessionMemorySource, Option<PathBuf>)> = Vec::new();
    ordered
        .into_iter()
        .map(|record| {
            let target = startup_restore_target(record, restored_panes, layout_restore_enabled);
            let target = match target {
                StartupRestoreTarget::Skip(_) => target,
                _ if record.native_session_id.is_some() => target,
                _ => {
                    let key = (record.source, record.cwd.clone());
                    if continued.contains(&key) {
                        StartupRestoreTarget::Skip(StartupRestoreSkip::DuplicateContinue)
                    } else {
                        continued.push(key);
                        target
                    }
                }
            };
            (record.id.clone(), target)
        })
        .collect()
}

fn startup_restore_target<W: Clone>(
    record: &SessionMemoryRecord,
    restored_panes: &[(W, Vec<u8>)],
    layout_restore_enabled: bool,
) -> StartupRestoreTarget<W> {
    let plan = match startup_agent_restore_plan(record) {
        Ok(plan) => plan,
        Err(err) => return StartupRestoreTarget::Skip(StartupRestoreSkip::Invalid(err)),
    };
    let restored = record.terminal_pane_uuid.as_ref().and_then(|uuid| {
        restored_panes
            .iter()
            .find(|(_, restored_uuid)| restored_uuid == uuid)
    });
    match restored {
        Some((window, terminal_pane_uuid)) => StartupRestoreTarget::ExistingPane {
            window: window.clone(),
            terminal_pane_uuid: terminal_pane_uuid.clone(),
            plan,
        },
        None if layout_restore_enabled => StartupRestoreTarget::Skip(StartupRestoreSkip::PaneGone),
        None => StartupRestoreTarget::NewTab { plan },
    }
}
```

- [ ] **Step 4: Run the tests (expect pass)**

Run: `cargo test -p warp --lib session_memory::restore`
Expected: all new tests pass. The existing restore tests still pass.

- [ ] **Step 5: Commit**

```bash
git add app/src/session_memory/restore.rs app/src/session_memory/restore_tests.rs
git commit -m "feat(session-memory): plan startup resume per restored pane"
```

---

## Task 7: Expose session creation time from the agent session reader

**Files:**
- Modify: `app/src/workspace/agent_session_reader.rs` (`AgentSessionEntry` ~6-14, Claude constructor ~166, `CodexThread` ~308-316, Codex query and constructor ~331-352)

**Interfaces:**
- Produces: `AgentSessionEntry::created_at: i64`. For Claude it is the first user message timestamp, else the file mtime, which is the value the existing code already computes into `updated_at`. For Codex it is `threads.created_at`. Verified: the `threads` table has `created_at INTEGER NOT NULL`, and `codex resume --help` documents `--last` as cwd-filtered (`--all` disables cwd filtering).
- Consumed by: Task 9 `agent_session_files`.

- [ ] **Step 1: Tests**

No new unit test. The reader reads real `~/.claude` / `~/.codex` data, and the file already contains a pre-existing inline `mod tests` (flagged by `script/check_no_inline_test_modules`). Adding tests here would extend that violation. The matching logic that consumes `created_at` is tested in Task 6. Do not move or edit the pre-existing inline module (out of scope).

- [ ] **Step 2: Baseline**

Run: `cargo check -p warp`
Expected: clean.

- [ ] **Step 3: Implement**

`AgentSessionEntry`: add `pub created_at: i64,` after `pub updated_at: i64,`.

Claude constructor in `parse_claude_session`: add `created_at: updated_at,` after `updated_at,`.

`CodexThread`: add

```rust
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    created_at: i64,
```

Query string becomes `"SELECT id, first_user_message, created_at, updated_at FROM threads WHERE cwd = ? ORDER BY updated_at DESC"`. In the Codex constructor, add `created_at: row.created_at,` after `updated_at: row.updated_at,`.

- [ ] **Step 4: Verify**

Run: `cargo check -p warp` and `bash script/check_no_inline_test_modules`.
Expected: the check is clean. The inline-module script lists exactly the three pre-existing files (`app/src/workspace/view/session_memory_transcript.rs`, `app/src/workspace/agent_session_reader.rs`, `app/src/hold_to_quit/mod.rs`) and nothing new.

- [ ] **Step 5: Commit**

```bash
git add app/src/workspace/agent_session_reader.rs
git commit -m "feat(workspace): expose agent session creation time"
```

---

## Task 8: Deferred run-or-insert restore command and the effective setting

**Files:**
- Modify: `app/src/terminal/view.rs` (field ~2634, init ~4413, `BootstrapPrecmdDone` arm ~13459-13462, `handle_session_bootstrapped` ~14234-14237, `execute_command_when_bootstrapped_or_defer` ~16689-16699)
- Modify: `app/src/terminal/view_tests.rs` (append)
- Modify: `app/src/settings/ai.rs` (~1914-1923)
- Modify: `app/src/settings/ai_tests.rs` (~541-544)

**Interfaces:**
- Consumes: `Input::replace_buffer_content(&mut self, content: &str, ctx)`, `Input::buffer_text(&self, ctx) -> String`, `Input::has_pending_command(&self) -> bool`, `test_util::terminal::{initialize_app_for_terminal_view, add_window_with_id_and_terminal}`.
- Produces:
  - `enum PendingSessionMemoryRestore { Run(String), Insert(String) }` (view.rs, private)
  - `TerminalView::insert_command_when_bootstrapped_or_defer(&mut self, command: &str, ctx: &mut ViewContext<Self>)`
  - `TerminalView::drain_pending_session_memory_restore(&mut self, ctx: &mut ViewContext<Self>)` (private)
  - The setting `session_memory_auto_restore_interrupted_sessions` defaults to `true`.

- [ ] **Step 1: Write the failing tests**

Append to `app/src/terminal/view_tests.rs`:

```rust
#[test]
fn deferred_session_memory_insert_fills_input_without_running_after_bootstrap() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let (_, terminal) = add_window_with_id_and_terminal(&mut app, None);
        let input = terminal.read(&app, |terminal, _ctx| terminal.input().clone());

        terminal.update(&mut app, |view, ctx| {
            view.is_login_shell_bootstrapped = false;
            view.insert_command_when_bootstrapped_or_defer("claude --continue", ctx);
        });
        assert!(input.read(&app, |input, ctx| input.buffer_text(ctx)).is_empty());

        terminal.update(&mut app, |view, ctx| {
            view.is_login_shell_bootstrapped = true;
            view.drain_pending_session_memory_restore(ctx);
        });
        assert_eq!(
            input.read(&app, |input, ctx| input.buffer_text(ctx)),
            "claude --continue"
        );
        assert!(!input.read(&app, |input, _ctx| input.has_pending_command()));
    });
}
```

In `app/src/settings/ai_tests.rs` `test_session_memory_settings_defaults`, change the assertion to:

```rust
            assert_eq!(
                *settings.session_memory_auto_restore_interrupted_sessions,
                true
            );
```

Reason: the spec makes the setting effective and on by default.

- [ ] **Step 2: Run the tests (expect failure)**

Run: `cargo test -p warp --lib deferred_session_memory_insert` and `cargo test -p warp --lib test_session_memory_settings_defaults`.
Expected: compile error `no method named insert_command_when_bootstrapped_or_defer`. The settings test fails with `left: false, right: true`.

- [ ] **Step 3: Implement**

`app/src/terminal/view.rs`:

Near the `TerminalView` struct (before `pub struct TerminalView`), add:

```rust
enum PendingSessionMemoryRestore {
    Run(String),
    Insert(String),
}
```

Replace the field `pending_session_memory_restore_command: Option<String>,` with `pending_session_memory_restore: Option<PendingSessionMemoryRestore>,`. Replace its initializer (`pending_session_memory_restore_command: None,`) with `pending_session_memory_restore: None,`.

In the `ModelEvent::BootstrapPrecmdDone` arm, replace

```rust
                if let Some(command) = self.pending_session_memory_restore_command.take() {
                    self.execute_command_or_set_pending(&command, ctx);
                }
```

with `self.drain_pending_session_memory_restore(ctx);`.

In `handle_session_bootstrapped`, replace

```rust
        if let Some(command) = self.pending_session_memory_restore_command.take() {
            log::info!("Session memory restore: executing deferred command after shell bootstrap");
            self.execute_command_or_set_pending(&command, ctx);
        }
```

with `self.drain_pending_session_memory_restore(ctx);`. The existing comment above it stays.

Replace `execute_command_when_bootstrapped_or_defer` with:

```rust
    pub fn execute_command_when_bootstrapped_or_defer(
        &mut self,
        command: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.is_login_shell_bootstrapped {
            self.execute_command_or_set_pending(command, ctx);
        } else {
            self.pending_session_memory_restore =
                Some(PendingSessionMemoryRestore::Run(command.to_string()));
        }
    }

    pub fn insert_command_when_bootstrapped_or_defer(
        &mut self,
        command: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.is_login_shell_bootstrapped {
            self.input
                .update(ctx, |input, ctx| input.replace_buffer_content(command, ctx));
        } else {
            self.pending_session_memory_restore =
                Some(PendingSessionMemoryRestore::Insert(command.to_string()));
        }
    }

    fn drain_pending_session_memory_restore(&mut self, ctx: &mut ViewContext<Self>) {
        match self.pending_session_memory_restore.take() {
            Some(PendingSessionMemoryRestore::Run(command)) => {
                log::info!("Session memory restore: executing deferred command after shell bootstrap");
                self.execute_command_or_set_pending(&command, ctx);
            }
            Some(PendingSessionMemoryRestore::Insert(command)) => {
                log::info!("Session memory restore: inserting deferred command after shell bootstrap");
                self.input
                    .update(ctx, |input, ctx| input.replace_buffer_content(&command, ctx));
            }
            None => {}
        }
    }
```

`app/src/settings/ai.rs`, `session_memory_auto_restore_interrupted_sessions`: `default: true,` and `description: "Relaunch Claude Code and Codex sessions that were open when Warp closed; when off, the resume command is inserted into the input.",`.

- [ ] **Step 4: Run the tests (expect pass)**

Run: `cargo test -p warp --lib deferred_session_memory_insert` and `cargo test -p warp --lib test_session_memory_settings_defaults`.
Expected: both pass.

- [ ] **Step 5: Commit**

```bash
git add app/src/terminal/view.rs app/src/terminal/view_tests.rs app/src/settings/ai.rs app/src/settings/ai_tests.rs
git commit -m "feat(session-memory): insert or run the resume command after bootstrap"
```

---

## Task 9: One-shot startup restore pass; remove per-window restore and enrichment

**Files:**
- Create: `app/src/workspace/view/session_memory_startup.rs`
- Modify: `app/src/workspace/view.rs` (module list lines 1-27; imports ~554-560; `Workspace::new` ~3784-3785; delete `enrich_session_memory_records_from_agent_index`, `enrich_session_memory_record_from_agent_index`, `auto_restore_startup_session_memory` and `restored_terminal_pane_targets` ~19547-19693; remove the enrichment call in `open_session_memory_board` ~19713; add `terminal_pane_session_uuids`)
- Modify: `app/src/lib.rs` (`launch`, after the `if ctx.window_ids().count() == 0 { ... }` block ~3176)
- Modify: `app/src/session_memory/restore.rs` (delete `StartupRestoreAction`, `RestoredTerminalPane`, `startup_restore_action_for_record`, `should_apply_restore_plan_to_existing_pane`)
- Modify: `app/src/session_memory/restore_tests.rs` (delete the old startup tests)
- Modify: `app/src/pane_group/mod.rs` (import line 123; delete `restored_terminal_pane_targets` ~1186-1204; `restored_cwd` ~1705 and `set_restored_cwd` ~1771)
- Modify: `app/src/pane_group/pane/terminal_pane.rs` (delete the `restored_cwd` field ~107-111, init ~297, `set_restored_cwd` / `restored_cwd` ~319-328)

**Interfaces:**
- Consumes: Task 5 `startup_restore_candidates`, `previous_run_native_session_ids`, `mark_startup_recovery_offered_and_notify`. Task 6 `plan_startup_restore`, `resolve_missing_session_ids`, `AgentSessionFile`, `StartupRestoreTarget`. Task 7 `AgentSessionEntry::created_at`. Task 8 `insert_command_when_bootstrapped_or_defer` / `execute_command_when_bootstrapped_or_defer`. `WorkspaceRegistry::{all_workspaces, get}`, `ctx.windows().{active_window, ordered_window_ids}`, `PaneGroup::terminal_pane_session_uuids`, and the private `Workspace::{terminal_view_for_session_uuid, open_terminal_for_restore_plan}` (reachable from a child module of `view`).
- Produces: `pub(crate) fn restore_open_agent_sessions(ctx: &mut AppContext)` at `crate::workspace::view::session_memory_startup`.

- [ ] **Step 1: Remove old-behavior tests**

Delete from `restore_tests.rs`, because the API they test (`startup_restore_action_for_record`, cwd fallback, `AlreadyRestoredPane`, startup terminal auto-run) is removed:
- `startup_restore_routes_agent_chat_to_existing_restored_pane`
- `startup_restore_does_not_duplicate_terminal_pane_restored_by_layout`
- `startup_restore_runs_auto_runnable_terminal_command_in_existing_restored_pane`
- `startup_restore_opens_new_pane_when_layout_did_not_restore_original_pane`
- `startup_restore_routes_agent_chat_to_restored_pane_with_matching_cwd_when_uuid_changed`
- `startup_restore_opens_new_pane_when_no_restored_pane_matches_uuid_or_cwd`
- `startup_restore_cwd_fallback_ignores_panes_without_known_cwd`
- the helper `restored_pane`

Also remove `RestoredTerminalPane, StartupRestoreAction, startup_restore_action_for_record` from its `use super::restore::{...}`. Their replacements (window targeting, pane-gone skip, new tab) are the Task 6 tests.

No new unit test for the glue module: every decision it makes is delegated to the Task 5 and Task 6 functions. Its wiring is verified by `cargo check`, clippy and the Task 10 manual scenario.

- [ ] **Step 2: Baseline**

Run: `cargo test -p warp --lib session_memory::restore`
Expected: passes after the deletions (old API still present).

- [ ] **Step 3: Implement**

Create `app/src/workspace/view/session_memory_startup.rs`:

```rust
use std::path::Path;

use warpui::{AppContext, SingletonEntity};

use crate::session_memory::model::SessionMemoryModel;
use crate::session_memory::restore::{
    AgentSessionFile, StartupRestoreTarget, plan_startup_restore, resolve_missing_session_ids,
};
use crate::session_memory::types::SessionMemorySource;
use crate::settings::AISettings;
use crate::terminal::CLIAgent;
use crate::terminal::general_settings::GeneralSettings;
use crate::workspace::WorkspaceRegistry;
use crate::workspace::agent_session_reader;

pub(crate) fn restore_open_agent_sessions(ctx: &mut AppContext) {
    if !ctx.has_singleton_model::<SessionMemoryModel>() {
        return;
    }
    let session_memory = SessionMemoryModel::as_ref(ctx);
    let mut candidates = session_memory.startup_restore_candidates();
    if candidates.is_empty() {
        return;
    }
    let workspaces = WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx);
    if workspaces.is_empty() || workspaces.len() < ctx.window_ids().count() {
        log::info!("Session memory startup restore: skipped, not every window has a workspace");
        return;
    }
    let claimed = session_memory.previous_run_native_session_ids();
    resolve_missing_session_ids(&mut candidates, claimed, agent_session_files);

    let restored_panes = workspaces
        .into_iter()
        .flat_map(|(window_id, workspace)| {
            workspace
                .as_ref(ctx)
                .terminal_pane_session_uuids(ctx)
                .into_iter()
                .map(move |uuid| (window_id, uuid))
        })
        .collect::<Vec<_>>();
    let layout_restore_enabled = *GeneralSettings::as_ref(ctx).restore_session;
    let run_resume_commands =
        *AISettings::as_ref(ctx).session_memory_auto_restore_interrupted_sessions;
    let first_window = ctx
        .windows()
        .active_window()
        .or_else(|| ctx.windows().ordered_window_ids().first().copied());
    let offered_ids = candidates
        .iter()
        .map(|record| record.id.clone())
        .collect::<Vec<_>>();

    for (record_id, target) in
        plan_startup_restore(&candidates, &restored_panes, layout_restore_enabled)
    {
        let (window_id, terminal_pane_uuid, plan) = match target {
            StartupRestoreTarget::ExistingPane {
                window,
                terminal_pane_uuid,
                plan,
            } => (Some(window), Some(terminal_pane_uuid), plan),
            StartupRestoreTarget::NewTab { plan } => (first_window, None, plan),
            StartupRestoreTarget::Skip(reason) => {
                log::info!("Session memory startup restore: skipping {record_id}: {reason:?}");
                continue;
            }
        };
        let Some(workspace) = window_id
            .and_then(|window_id| WorkspaceRegistry::as_ref(ctx).get(window_id, ctx))
        else {
            log::warn!("Session memory startup restore: no window for {record_id}");
            continue;
        };
        let terminal_view = workspace.update(ctx, |workspace, ctx| match &terminal_pane_uuid {
            Some(uuid) => workspace.terminal_view_for_session_uuid(uuid, ctx),
            None => workspace.open_terminal_for_restore_plan(&plan, false, ctx),
        });
        let (Some(terminal_view), Some(command)) =
            (terminal_view, plan.command().map(str::to_owned))
        else {
            log::warn!("Session memory startup restore: no terminal for {record_id}");
            continue;
        };

        log::info!("Session memory startup restore: {record_id} -> {command}");
        terminal_view.update(ctx, |terminal_view, ctx| {
            if run_resume_commands {
                terminal_view.execute_command_when_bootstrapped_or_defer(&command, ctx);
            } else {
                terminal_view.insert_command_when_bootstrapped_or_defer(&command, ctx);
            }
        });
    }

    SessionMemoryModel::handle(ctx).update(ctx, |model, ctx| {
        model.mark_startup_recovery_offered_and_notify(&offered_ids, ctx);
    });
}

fn agent_session_files(source: SessionMemorySource, cwd: &Path) -> Vec<AgentSessionFile> {
    let agent = match source {
        SessionMemorySource::ClaudeCode => CLIAgent::Claude,
        SessionMemorySource::Codex => CLIAgent::Codex,
        SessionMemorySource::WarpTerminal => return Vec::new(),
    };
    agent_session_reader::read_all_sessions(agent, cwd)
        .into_iter()
        .map(|entry| AgentSessionFile {
            session_id: entry.session_id,
            created_at: entry.created_at,
        })
        .collect()
}
```

`app/src/workspace/view.rs`:
1. Add `pub(crate) mod session_memory_startup;` after `pub(crate) mod session_memory_board;` (line 18).
2. Delete the two lines `ws.enrich_session_memory_records_from_agent_index(ctx);` and `ws.auto_restore_startup_session_memory(ctx);` in `Workspace::new`.
3. Delete `enrich_session_memory_records_from_agent_index`, `enrich_session_memory_record_from_agent_index`, `auto_restore_startup_session_memory` and `restored_terminal_pane_targets`. Delete the line `self.enrich_session_memory_records_from_agent_index(ctx);` in `open_session_memory_board`.
4. Add next to `terminal_view_for_session_uuid`:

```rust
    fn terminal_pane_session_uuids(&self, ctx: &AppContext) -> Vec<Vec<u8>> {
        self.tabs
            .iter()
            .flat_map(|tab| tab.pane_group.as_ref(ctx).terminal_pane_session_uuids())
            .collect()
    }
```

5. Imports (~554-560): remove `RestoredTerminalPane`, `StartupRestoreAction` and `startup_restore_action_for_record`. Remove any of `SessionMemoryKind` / `SessionMemorySource` / `CLIAgent` imports that `cargo check` reports unused.

`app/src/lib.rs`, in `launch`, right after the block

```rust
            if ctx.window_ids().count() == 0 {
                ctx.dispatch_global_action("root_view:open_new", &());
            }
```

add:

```rust
            if matches!(launch_mode, LaunchMode::App { .. }) {
                crate::workspace::view::session_memory_startup::restore_open_agent_sessions(ctx);
            }
```

`app/src/session_memory/restore.rs`: delete `StartupRestoreAction`, `RestoredTerminalPane`, `startup_restore_action_for_record` and `should_apply_restore_plan_to_existing_pane`.

`app/src/pane_group/mod.rs`: delete `use crate::session_memory::restore::RestoredTerminalPane;` and `pub fn restored_terminal_pane_targets(...)`. In the restore-from-snapshot code, delete `let restored_cwd = startup_directory.clone();` and `pane_data.set_restored_cwd(restored_cwd);`. Change `let mut pane_data = TerminalPane::new(` to `let pane_data = TerminalPane::new(` if `cargo check` reports `unused_mut`.

`app/src/pane_group/pane/terminal_pane.rs`: delete the `restored_cwd` field and its doc comment, the `restored_cwd: None,` initializer, and the methods `set_restored_cwd` and `restored_cwd`.

- [ ] **Step 4: Verify**

Run: `cargo check -p warp`, `cargo test -p warp --lib session_memory`, then `cargo clippy -p warp --all-targets --tests -- -D warnings`.
Expected: clean. All session_memory tests pass.

- [ ] **Step 5: Commit**

```bash
git add app/src/workspace/view/session_memory_startup.rs app/src/workspace/view.rs app/src/lib.rs app/src/session_memory/restore.rs app/src/session_memory/restore_tests.rs app/src/pane_group/mod.rs app/src/pane_group/pane/terminal_pane.rs
git commit -m "fix(session-memory): restore agent sessions once after all windows open"
```

---

## Task 10: Full verification and manual scenario

**Files:** none changed. If a check fails, fix it in the owning task's files and amend with a new commit.

- [ ] **Step 1: Format and static checks**

Run:
```bash
./script/format --check
bash script/check_no_inline_test_modules
cargo clippy -p warp --all-targets --tests -- -D warnings
```
Expected: format clean and clippy clean. The inline-module script lists exactly the three pre-existing files named in Task 7 and nothing else.

- [ ] **Step 2: Targeted tests**

Run:
```bash
cargo test -p warp --lib session_memory
cargo test -p warp --lib persistence::sqlite
cargo test -p warp --lib workspace::view::session_memory_board
cargo test -p warp --lib deferred_session_memory_insert
cargo test -p warp --lib test_session_memory_settings_defaults
cargo test -p warp --lib cli_agent
```
Expected: all pass.

- [ ] **Step 3: Full library suite**

Run: `cargo test -p warp --lib`
Expected: pass. About 12 tests are known to flake only when the whole suite runs in one process. Rerun any failure by exact name (`cargo test -p warp --lib <test_name> -- --exact`). Accept it only if it passes in isolation and is unrelated to session memory, pane_group, persistence or terminal view. List those names in the PR description.

- [ ] **Step 4: Manual scenario on the `fc` dev profile**

Build: `cargo build -p warp --bin warp-oss && cargo build -p warp_fork_control`.
DB for inspection: `DB=~/Library/Group\ Containers/*.dev.warp/Library/Application\ Support/dev.warp.WarpOss-fc/warp.sqlite` (use `sqlite3 -readonly "$(ls $DB)"`).

1. Launch: `WARP_DATA_PROFILE=fc ./target/debug/warp-oss &`, and log in if prompted.
2. Create sessions with the fork CLI (`export WARP_DATA_PROFILE=fc`):
   - `./target/debug/warp-fork-ctl open --cwd "$PWD" --cmd "claude" --title W1-waiting`. Leave it at its prompt (waiting for input).
   - `./target/debug/warp-fork-ctl open --cwd "$PWD" --cmd "codex" --title W1-codex`
   - `./target/debug/warp-fork-ctl open --cwd "$PWD" --cmd "claude" --title W2-working --new-window`. Then `warp-fork-ctl send <pane_id> "count slowly to 200" --submit` so it is working at close time.
   - `./target/debug/warp-fork-ctl open --cwd "$PWD" --cmd "claude" --title W2-exited`. Then `warp-fork-ctl send <pane_id> "/exit" --submit` so the agent exits and the pane stays open.
   - `./target/debug/warp-fork-ctl list --json`: record the `window_id`, `pane_id` and `running_command` per title.
3. DB checks before close:
   - Immediately after the W2-exited `/exit`, before any other action, run `select source, status, started_at, completed_at from session_memory_records where id = '<W2-exited record id>';`. Expect `claude_code`, `success`, and both `started_at` and `completed_at` set. This proves the ended UPDATE matched the block start. A later snapshot may rewrite the row as `warp_terminal`, so do not accept that as evidence.
   - Then run `select id, source, kind, status, native_session_id, started_at, completed_at, app_run_id from session_memory_records order by last_seen_at desc limit 8;`. Expect three `claude_code`/`codex` rows with `started_at` NOT NULL and `completed_at` NULL.
4. Force quit: `kill -9 <warp-oss pid>`. Relaunch with `WARP_DATA_PROFILE=fc ./target/debug/warp-oss &`.
   - Expect both windows to come back. W1-waiting, W1-codex and W2-working each run their resume command in their own pane and window: `claude --resume <id>` when the id is known, otherwise `claude --continue` / `codex resume --last`.
   - W2-exited stays a plain shell.
   - `warp-fork-ctl list --json` shows `running_command` for exactly those three panes.
5. Cmd-Q: quit via the app menu (normal quit). Before relaunch, check the DB:
   - `select count(*) from session_memory_records where app_run_id = (select run_id from session_memory_app_runs order by started_at desc limit 1) and source in ('claude_code','codex') and completed_at is null and closed_intentionally_at is null;`
   - Expect 3, which proves shutdown neither ended nor closed the rows. Relaunch and expect the same three sessions back.
6. Relaunch twice: quit, then in each window close the three agent panes' agents with `/exit` (or Ctrl-C). Then Cmd-Q and relaunch. Expect nothing to be relaunched. Cmd-Q and relaunch once more. Expect nothing from the run before the last.
7. Setting off: set "Automatically restore interrupted sessions" off (Settings, CLI agents). Start one `claude`, `kill -9`, relaunch. Expect the pane input to contain the resume command, not running. Turn the setting back on.
8. Layout restore off (spec Startup rule 5): turn off Settings, Features, "Restore windows, tabs, and panes on startup" (`general.restore_session`). Start `claude` in one tab and wait until the footer shows the agent. Check that its row exists with `source = 'claude_code'` and `completed_at` NULL (written by the CLI-agent event path, because snapshots do not run in this mode). Then `kill -9` and relaunch. Expect one new tab in the first window running the resume command in the record's cwd. Turn the setting back on.
9. Logged-out guard: sign out, start nothing, `kill -9`, relaunch. Expect the log line `Session memory startup restore: skipped, not every window has a workspace` when previous-run candidates exist, and no crash. Sign back in.
10. `select count(*) from session_memory_app_runs;` before and after running `./target/debug/warp-oss` CLI subcommands (for example `WARP_DATA_PROFILE=fc ./target/debug/warp-oss --help` or any `LaunchMode::CommandLine` invocation). Expect the count to be unchanged.

- [ ] **Step 5: Codex review gate**

Run `/codex:review --base master` (non-trivial, data-handling change). Address findings in the owning task's files with follow-up commits.

---

## Rulings

1. **Lifecycle is stored in existing columns.** `completed_at IS NULL` plus `status = live` means the agent is live. `completed_at` set plus `status = success` means it ended. Turn state (idle, blocked, working) is no longer persisted.
   - Why: the existing columns express the lifecycle without a schema change, and the migration rule bars editing migrations. The status column was the only turn-state carrier and caused the idle/blocked skip.
   - Cost if wrong: the board loses its "blocked" badge. It can be re-added in `restore_payload` without affecting restore.
2. **Id source (3), the session-file match, is resolved at startup, not at record time.**
   - Why: a filesystem scan on every pane snapshot is too expensive. The "claimed by another pane" set is only complete once all previous-run records are known.
   - Cost if wrong: an id-less candidate whose session file shows up late gets `--continue` in its cwd, the spec's own fallback.
3. **The ended key is the pane record id plus the agent block's start second.** Both sides use `block_timestamp_seconds`, and `SerializedBlock.start_ts` is copied from `Block.start_ts`.
   - Why: the pane knows its uuid, and the block start identifies one agent run, so a later snapshot of the same run keeps the end while a new agent (new start) clears it.
   - Cost if wrong: a timestamp mismatch leaves the end unpersisted. That is a silent regression of the core defect, and `record_started_at_matches_block_timestamp_seconds` guards it.
4. **The candidate predicate ignores `status`.** It is: agent source, previous run, `completed_at IS NULL`, `closed_intentionally_at IS NULL`, and not offered.
   - Why: rows written by older builds carry `success`/`blocked` for open agents.
   - Cost if wrong: an older-build agent that actually exited, whose end the old code never persisted, may be relaunched once at the upgrade boundary.
5. **A plugin session with an unrecognized command (alias such as `cx`) is recorded as an agent** with `AgentPermissionMode::Unknown`.
   - Why: the plugin is authoritative about the running agent. The spec's "derive from the running command" targets the `last_command` bug.
   - Cost if wrong: an alias that wraps flags loses the dangerous flag on resume.
6. **Startup tmux/terminal auto-run is removed.**
   - Why: the spec says plain terminals are unchanged and their commands are not re-run. The old startup path auto-ran `tmux attach`, while the board's manual restore keeps it.
   - Cost if wrong: tmux users re-attach by hand after restart.
7. **Two id-less candidates for the same agent and cwd: only the newest (`last_seen_at`) runs `--continue`, the rest are skipped** (their panes stay shells).
   - Why: both would open the same conversation. `codex resume --last` is cwd-filtered, verified with `codex resume --help` ("`--all` disables cwd filtering").
   - Cost if wrong: the user relaunches the second agent by hand.
8. **Codex id parsing takes only the token right after `resume`.** Options placed before the id yield no id and therefore `codex resume --last`.
   - Why: `codex resume [OPTIONS] [SESSION_ID]` options take values (`-c key=val`), and guessing mis-parses them as ids.
   - Cost if wrong: that rare form resumes the latest cwd session instead of the exact one.
9. **The offered marker uses a targeted `UPDATE ... WHERE id IN (...) AND app_run_id = <previous>`** in place of the old full-row upsert of the stale in-memory record.
   - Why: restored panes reuse their uuid and may have already written a current-run row that the full-row write would clobber.
   - Cost if wrong: none identified. The marker is only an extra guard, because the pass runs once per process.
10. **Hook point.** `root_view:open_from_restored` runs synchronously inside `lib.rs::launch`, so every snapshot window and pane exists once it returns. The pass runs there, only for `LaunchMode::App`.
    - Why: this is the smallest point that is after all windows exist and before any user action.
    - Guard: `RootView::new` builds the `Workspace` synchronously only when logged in. When logged out or onboarding, it is deferred until auth, so the pass returns early, without marking anything offered, whenever `WorkspaceRegistry` holds fewer workspaces than open windows. Retrying after auth was rejected: there are several post-auth workspace-creation paths (`complete_auth_and_create_workspace`, `LoginSlide`, onboarding), and none is a single hook.
    - Cost if wrong: a launch that starts logged out does not auto-restore. The records stay on the board for manual restore.
11. **Upgrade boundary.** Non-App run rows already in the database can make "previous run" point at a CLI/proxy run on the first launch after this change, so nothing is restored that once.
    - Why: filtering by "runs that own records" could resurrect older runs, which violates the spec.
    - Cost if wrong: one missed restore after upgrading.
12. **Ending an agent is skipped while `ApplicationStage::Terminating`.** The SQLite writer is also joined in `on_will_terminate` before terminal teardown.
    - Why: a SIGHUP'd agent must not look like `/exit`.
    - Cost if wrong: Cmd-Q sessions would not come back. Task 10 step 5 checks the rows directly.
13. **Setting semantics.** `session_memory_auto_restore_interrupted_sessions` (default `true`) decides run versus insert for the startup pass. `session_memory_auto_run_restored_commands` keeps its board-only meaning.
    - Why: the spec names only the first setting.
    - Cost if wrong: the board's restore behaviour is unchanged, so it is low.
14. **`RestoredTerminalPane`, its cwd fallback and the `restored_cwd` plumbing are removed.**
    - Why: the spec matches pane uuids exactly, and the cwd fallback only existed for the removed guessing.
    - Cost if wrong: none for restore. These fields had no other reader.
15. **The pre-existing inline test modules are left alone:** `session_memory_transcript.rs`, `agent_session_reader.rs`, `hold_to_quit/mod.rs`.
    - Why: this is a surgical change. `script/check_no_inline_test_modules` already fails on master for these files.
    - Cost if wrong: presubmit keeps failing on them until they are moved in a separate change.
16. **With layout restore off, agent records come only from the CLI-agent event path.** `workspace:save_app` returns early when `restore_session` is off, so pane snapshots, and with them the snapshot upsert, do not run. The `CLIAgentSessionsModel` subscription (Task 4) still writes the full record on `Started` / `StatusChanged` / `SessionUpdated`, and the ended UPDATE is not gated on `restore_session`.
    - Why: this makes spec Startup rule 5 reachable without touching the `save_app` gate.
    - Cost if wrong: if no event fires for an agent, it is not restored in that mode. Task 10 step 8 checks the row directly.
