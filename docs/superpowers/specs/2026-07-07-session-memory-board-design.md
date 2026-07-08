# Session Memory Board and Interrupted Session Recovery - Design Spec

**Date:** 2026-07-07
**Branch:** `feat/session-memory-board`
**Base branch:** `feat/remote-control`
**Status:** Draft

---

## Problem

Warp can restore ordinary app state, and `feat/remote-control` adds a good flow
for resuming Claude Code and Codex sessions from the `+` menu. The missing piece
is a single place that answers:

- Which terminal sessions were open when Warp or the machine stopped?
- Which sessions were closed intentionally by the user?
- Which Claude Code or Codex chats exist for this project?
- How do I resume the right terminal, Claude Code, or Codex context without
  remembering which pane, tab, or chat file was involved?

After a reboot, Warp cannot resurrect the original PTY process. It can only
restore layout, working directory, last known command context, and offer safe
resume or rerun actions. The design should be explicit about that boundary.

---

## Goals

1. Track terminal pane lifecycle separately from ordinary app-state snapshots,
   so Warp can distinguish user-closed sessions from interrupted sessions.
2. Add a Session Memory Board that shows interrupted terminal sessions, live
   CLI agent sessions, and indexed Claude Code / Codex chat history together.
3. Support both startup recovery prompts and opt-in automatic restoration.
4. Reuse the existing `feat/remote-control` Claude/Codex session resume work
   instead of introducing a competing reader or resume path.
5. Keep dangerous behavior opt-in. Restored commands must not auto-run unless a
   dedicated setting is enabled.
6. Preserve the effective agent permission mode across restore. If a Claude
   Code or Codex session was launched with dangerous permission/sandbox-bypass
   flags, restoring that same session must include the same flags, with clear UI
   labeling before launch.

---

## Non-Goals

- No true process resurrection after reboot. The original PTY and child
  processes are gone.
- No semantic embeddings or RAG in the first implementation.
- No cloud sync for local terminal recovery records.
- No automatic execution of recovered commands by default.
- No broad support for every CLI agent transcript format in the first pass.
  Claude Code and Codex are first because this branch already has readers and
  native resume commands for them.

---

## Current Starting Point

The base branch already contains useful pieces:

- `app/src/workspace/agent_session_reader.rs`
  - Reads recent Claude Code and Codex sessions for a directory.
  - Provides `read_all_sessions`, `read_sessions`, and `source_version`.
  - Caches cheaply by source mtime in `Workspace`.
- `app/src/terminal/cli_agent.rs`
  - Adds `supports_resume()`, `resume_command()`,
    `resume_command_with_flags()`, `launch_command()`, and dangerous flag
    helpers for Claude Code and Codex.
- `app/src/workspace/view.rs`
  - Adds the L3 sessions sub-sidecar in the `+` menu.
  - Adds launch/resume helpers that open a new tab in a directory and run the
    selected agent command.
- Existing persistence:
  - `AppState`, `WindowSnapshot`, `TabSnapshot`, and `TerminalPaneSnapshot`.
  - `terminal_panes` table stores terminal UUID, cwd, active state, launch data,
    input config, and related agent conversation IDs.
  - `blocks` and `commands` tables store completed terminal work.

These pieces are not enough to answer interrupted-session questions because
they do not model terminal lifecycle explicitly.

---

## UX

### Startup Behavior

On app start, Warp checks the local recovery registry.

If interrupted sessions exist and `show_recovery_board_on_startup` is enabled,
open a Session Memory Board in the first restored window.

If `auto_restore_interrupted_sessions` is enabled, restore interrupted terminal
windows/tabs/panes automatically, then show a compact banner:

```text
Restored 3 interrupted terminal sessions.
Commands were not run automatically.
```

If `auto_run_restored_commands` is disabled, any recovered command-like action
is presented behind an explicit "Run" or "Press Enter to run" affordance.

### Session Memory Board

The board is a Warp pane or modal with tabs/filters:

```text
Session Memory

[All] [Interrupted] [Terminal] [Claude Code] [Codex] [Live]

Interrupted Terminal Sessions
  20:51  ~/projects/warp                  last: cargo check -p warp
        Restore window    Restore tab    Copy last command

Live CLI Agent Sessions
  Claude Code  blocked  ~/projects/warp   waiting for permission
        Focus pane       Open composer

AI Chat History
  Codex        20:37  ~/projects/warp      "session memory board design"
        Resume          Open transcript
  Claude Code  18:12  ~/projects/dotfiles  "Atuin and Warp integration"
        Resume          Open transcript
```

The board should be reachable from:

- Command palette: `Show Session Memory`
- Startup recovery prompt
- Optional `+` menu entry after the existing agent launch items

### Row Actions

Terminal interrupted row:

- Restore window/tab/pane
- Restore in new tab
- Copy last command
- Delete recovery record

Claude/Codex row:

- Resume in new tab
- Resume in split pane, using remote-control-style workspace helpers when
  available
- Resume from the session's saved working directory. Warp should `cd` or open
  the new pane at the saved `cwd` before running the agent resume command.
- Preserve saved launch flags for sessions created through Warp, including
  dangerous permission/sandbox-bypass flags. Rows with those flags show a
  privileged/dangerous badge and use the confirmation policy described below.
- Open transcript viewer, initially as a read-only text/transcript pane
- Delete local board index row only, without deleting the source transcript

Live CLI agent row:

- Focus existing pane
- Open rich input composer when available
- Copy session ID or transcript path

---

## Data Model

Introduce a local `session_memory_records` table. This is a derived index and
registry, not the source of truth for transcripts.

```sql
CREATE TABLE session_memory_records (
    id TEXT PRIMARY KEY NOT NULL,
    source TEXT NOT NULL,
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT,
    cwd TEXT,
    project TEXT,
    native_session_id TEXT,
    transcript_path TEXT,
    terminal_pane_uuid BLOB,
    app_window_fingerprint TEXT,
    app_tab_fingerprint TEXT,
    last_command TEXT,
    last_exit_code INTEGER,
    launch_argv TEXT,
    permission_mode TEXT,
    last_seen_at INTEGER NOT NULL,
    started_at INTEGER,
    completed_at INTEGER,
    closed_intentionally_at INTEGER,
    restore_payload TEXT
);
```

Field meanings:

- `source`: `warp_terminal`, `claude_code`, `codex`, or later another agent.
- `kind`: `terminal` or `agent_chat`.
- `status`: `live`, `blocked`, `success`, `user_closed`, `interrupted`,
  `stale`, or `unknown`.
- `native_session_id`: Claude/Codex session UUID when available.
- `transcript_path`: local source transcript path when available.
- `terminal_pane_uuid`: existing Warp terminal pane UUID.
- `restore_payload`: JSON for source-specific restore instructions.
- `launch_argv`: JSON array of the effective argv Warp used when it launched or
  resumed the session, when known.
- `permission_mode`: `normal`, `dangerous`, or `unknown`. `dangerous` means the
  restored agent command must carry the same dangerous permission/sandbox-bypass
  flags that were present in `launch_argv` or source-specific restore metadata.

The table is intentionally flat. UI listing and filtering should not need to
join through the full app-state pane tree.

---

## Runtime Model

Add a singleton model:

```rust
pub struct SessionMemoryModel {
    records: Vec<SessionMemoryRecord>,
    model_event_sender: Option<SyncSender<ModelEvent>>,
}
```

Responsibilities:

- Load records from SQLite during persistence initialization.
- Upsert terminal lifecycle records.
- Upsert records from `CLIAgentSessionsModel` events.
- Trigger background refresh of Claude/Codex local transcript records.
- Provide filtered views for the board.
- Emit model events when rows are added, updated, or removed.

The model should avoid holding terminal model locks. Terminal-specific details
must be captured at the existing snapshot/event boundaries where the code
already has access to pane metadata.

---

## Terminal Recovery Lifecycle

### Recording

On terminal pane snapshot, write or update a `warp_terminal` record:

- `terminal_pane_uuid`
- current `cwd`
- active shell launch data serialized into `restore_payload`
- last known command from the active or most recent completed block where
  available
- `last_seen_at = now`
- `status = live`

On explicit pane/tab/window close, mark matching records:

- `status = user_closed`
- `closed_intentionally_at = now`

On shell exit, mark:

- `status = success` when the shell exited cleanly after bootstrap
- `status = interrupted` only when the app starts and sees a previously-live
  record that was not intentionally closed

### Startup Classification

During startup:

1. Load `session_memory_records`.
2. Any `warp_terminal` record with `status = live` and no
   `closed_intentionally_at` becomes `interrupted`.
3. Records already marked `user_closed` stay hidden from the default recovery
   view.
4. Old interrupted records are retained until the user deletes them or a future
   retention policy removes them.

### Restore

Restoring an interrupted terminal session creates a new terminal pane using:

- saved cwd
- saved shell launch data when available
- optional last command copied into the input buffer, not executed

Automatic restoration opens panes/tabs/windows but still does not run commands
unless `auto_run_restored_commands` is explicitly enabled.

---

## Claude and Codex Chat Indexing

The existing `agent_session_reader` should be promoted or wrapped by a broader
session-memory reader.

Required additions to `AgentSessionEntry`:

```rust
pub struct AgentSessionEntry {
    pub session_id: String,
    pub title: String,
    pub updated_at: i64,
    pub cwd: Option<PathBuf>,
    pub transcript_path: Option<PathBuf>,
    pub source: CLIAgent,
    pub launch_argv: Option<Vec<String>>,
    pub permission_mode: AgentPermissionMode,
}
```

`AgentPermissionMode` should distinguish at least `Normal`, `Dangerous`, and
`Unknown`. For sessions launched through Warp, this is derived from the actual
command flags used at launch/resume time. For externally indexed transcripts,
Warp must not invent dangerous flags when the original invocation is unknown.

Claude Code:

- Source remains `~/.claude/projects/<slug>/*.jsonl`.
- Store transcript path for direct open/copy actions.
- Keep custom-title, ai-title, and first-user-message priority from the
  existing branch.

Codex:

- Source remains `~/.codex/state_5.sqlite` `threads` table.
- Use rollout JSONL discovery for `transcript_path` when possible:
  `~/.codex/sessions/YYYY/MM/DD/rollout-*-<session_id>.jsonl`.
- Keep read-only SQLite URI behavior.

Board indexing should not copy transcript contents into Warp's database in the
first implementation. Store metadata and read the transcript lazily when the
user opens it.

### Agent Restore Semantics

Restoring a Claude Code or Codex row creates a new Warp tab or split pane at
the row's saved `cwd`, then runs the source-specific resume command for the
stored `native_session_id`.

For sessions originally launched or resumed through Warp, restore uses the
saved effective invocation metadata:

- preserve the original agent binary and resume subcommand shape where possible
- preserve the original dangerous permission/sandbox-bypass flags
- preserve other explicit launch flags that affect agent behavior
- append or replace only the session identifier needed for resume

If the saved `cwd` no longer exists, Warp shows a restore error with an action
to pick a replacement directory. It must not silently resume from the current
Warp directory.

If `permission_mode = dangerous`, the board row and startup recovery prompt
show a privileged/dangerous badge. Auto-restoring such rows may recreate the tab
and working directory, but auto-running the dangerous resume command requires an
explicit confirmation setting or a per-restore user click.

---

## Settings

Add settings under `agents.session_memory`:

```toml
[agents.session_memory]
enabled = true
show_recovery_board_on_startup = true
auto_restore_interrupted_sessions = false
auto_run_restored_commands = false
index_claude_code = true
index_codex = true
```

Defaults:

- Tracking enabled.
- Startup board enabled.
- Auto-restore disabled.
- Auto-run disabled.
- Claude/Codex indexing enabled.

Dangerous Claude/Codex launch flags remain controlled by the existing
per-agent settings from `feat/remote-control`.

For new sessions, those per-agent settings define the effective launch flags.
For restoring an existing Warp-created agent session, saved `launch_argv` and
`permission_mode` take precedence so that a session launched with dangerous
permissions resumes with the same permission mode even if defaults changed
later.

---

## Implementation Phases

### Phase 1 - Terminal Recovery Registry

Add the database table, persistence model types, and `ModelEvent` variants:

- `UpsertSessionMemoryRecord`
- `MarkSessionMemoryRecordClosed`
- `DeleteSessionMemoryRecord`
- `LoadSessionMemoryRecords` is not an event; loading happens with persisted
  data initialization.

Wire terminal pane close/snapshot events to write lifecycle records.

Verification:

- Unit tests for status transitions.
- SQLite round-trip tests for records.
- `cargo check -p warp`.

### Phase 2 - Startup Recovery Board

Add `SessionMemoryModel` and a basic board UI for interrupted terminal records.

Actions:

- Restore in new tab.
- Copy last command.
- Delete record.

Verification:

- App/model tests for startup classification.
- Manual run with an injected interrupted record.

### Phase 3 - Claude/Codex Board Integration

Extend `agent_session_reader` metadata and surface Claude/Codex records in the
board. Reuse existing resume command helpers.

Actions:

- Resume in new tab.
- Resume from the saved `cwd`, not from the currently focused terminal
  directory.
- Preserve saved launch flags, including dangerous permission flags, for
  Warp-created sessions.
- Open transcript path in a read-only viewer or external editor fallback.

Verification:

- Unit tests for Claude parser and Codex reader.
- Manual read-only test against local `~/.claude` and `~/.codex` sources.

### Phase 4 - Auto-Restore

Add startup behavior controlled by settings:

- show board
- auto-restore interrupted terminal layout
- never auto-run commands unless the explicit setting is enabled

Verification:

- Unit tests for settings gates.
- Manual restart simulation with interrupted records.

### Phase 5 - Search and Polish

Add board search/filtering across:

- title
- cwd/project
- last command
- source
- status

This phase can use SQLite FTS later, but the first version can filter in memory
over bounded metadata rows.

---

## Error Handling

- Missing Claude/Codex sources produce no rows and no blocking UI error.
- Corrupt transcript files are skipped with a warning log.
- SQLite write failures log errors and do not block terminal use.
- Restore failures show a toast with the source and reason.
- Transcript delete actions never delete source transcript files in this spec.

---

## Privacy and Safety

- All data stays local.
- Do not store transcript bodies in the new table during the first
  implementation.
- Store command text because Warp already stores command/block history locally;
  respect existing command-history filtering if available.
- Auto-run is disabled by default and separated from auto-restore.
- Dangerous Claude/Codex flags remain opt-in, visible in settings, and visible
  on any board row whose restore command would include them.
- Warp must not silently drop dangerous flags from a session that was launched
  with them, because that changes the resumed agent's behavior. It must also
  not silently add dangerous flags when the original invocation is unknown.

---

## Design Decisions

1. Board surface: implement the first version as an in-app pane opened from the
   command palette and startup recovery prompt. A left-panel tool can come
   later if the board becomes a primary navigation surface.
2. Retention: interrupted records persist until user deletion in the first
   version. A later setting can expire records after N days.
3. Transcript viewer: first version opens transcripts through the simplest
   existing local-file/read-only text surface. A richer transcript renderer is a
   separate follow-up.
4. Dismissing indexed Claude/Codex rows hides the derived board row, not the
   source transcript. The source can reappear after reindexing unless a future
   dismissed-record table is added.
5. Remote sessions: local terminal recovery records remote host metadata when
   available, but remote process recovery remains out of scope.

---

## Acceptance Criteria

- User-closing a terminal pane prevents that pane from appearing as interrupted
  after restart.
- Killing Warp or rebooting while a terminal pane is live causes that pane to
  appear as interrupted on next launch.
- The recovery board can restore an interrupted terminal context without
  auto-running commands.
- The board lists Claude Code and Codex sessions for known project directories
  using existing local sources.
- Claude/Codex board rows can resume sessions using the existing
  `resume_command_with_flags` path.
- Claude/Codex restore opens in the saved `cwd` and includes saved dangerous
  permission/sandbox-bypass flags when the original Warp-created session had
  them.
- `cargo check -p warp` passes.
