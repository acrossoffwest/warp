# CLI Agent Session Resume — Design Spec

**Date:** 2026-05-13  
**Branch:** `feat/remote-control`  
**Status:** Draft

---

## Problem

When the user opens the `+` menu and picks Codex or Claude Code, a directory picker sidecar appears. Selecting a directory immediately starts a **new** session. There is no way to resume a previous session from the same UI flow.

---

## Goal

Add a third navigation level: hovering a directory in the sidecar opens a **sessions sub-sidecar** showing recent sessions for that directory. Clicking a session resumes it; clicking the directory itself still starts a new session.

---

## Navigation Flow

```
Level 1 — + dropdown
  ⚡ Codex ›
  🤖 Claude Code ›
        │ hover
        ▼
Level 2 — directory sidecar  (existing, extended)
  📁 ~/projects/warp ›      ← click = new session; hover = open L3
  📁 ~/projects/foo  ›
        │ hover
        ▼
Level 3 — sessions sub-sidecar  (NEW)
  🔍 [filter sessions...]
  ──────────────────────
  ▶ New session           ← always first; click = new session in this dir
  ──────────────────────
  🕐 13 май 14:15  Find Codex and Claude chats     ← click = resume
  🕐 11 май 07:52  Add tmux monitoring pane
  🕐 11 май 04:29  Find price display command
  (up to 10, filtered by query)
```

---

## Session Data Sources

### Claude Code
- **Location:** `~/.claude/projects/<cwd-hash>/*.jsonl`
- **cwd-hash:** SHA256 of the directory path, then hex-encoded, matching existing Claude Code convention: `<path>.replace('/', '-').replace('~', '-Users-<user>')` (empirically: `-Users-<user>-projects-...-<repo>`).
- **Session ID:** JSONL filename stem (UUID), used as `--resume` argument.
- **Title:** First entry with `"type": "ai-title"` → `.aiTitle` field. Fallback: first `"type": "user"` message text, first 80 chars.
- **Timestamp:** First `"type": "user"` entry → `.timestamp` field (ISO-8601).
- **Resume command:** `claude --resume <session-id>`

### Codex
- **Location:** `~/.codex/state_5.sqlite` → `threads` table.
- **Filter:** `WHERE cwd = ?` (exact match on absolute path).
- **Order:** `ORDER BY updated_at DESC LIMIT 10`.
- **Session ID:** `id` column (UUID7); matches rollout JSONL filename.
- **Title:** `first_user_message` column, first 80 chars. No auto-title equivalent.
- **Timestamp:** `updated_at` column (Unix seconds).
- **Resume command:** `codex --resume <session-id>`

---

## New Types

### `AgentSessionEntry`
```rust
pub struct AgentSessionEntry {
    pub session_id: String,
    pub title: String,       // ai-title or first_user_message[:80]
    pub updated_at: i64,     // Unix timestamp seconds
}
```

### `SessionSidecarSelection` (new enum)
```rust
enum SessionSidecarSelection {
    NewSession  { agent: CLIAgent, directory: PathBuf },
    ResumeSession { agent: CLIAgent, directory: PathBuf, session_id: String },
}
```

---

## New Module: `agent_session_reader`

**Path:** `app/src/workspace/agent_session_reader.rs`

```rust
pub fn read_sessions(
    agent: CLIAgent,
    directory: &Path,
    query: &str,         // filter string (case-insensitive substring)
    limit: usize,        // max results after filtering
) -> Vec<AgentSessionEntry>
```

Behaviour:
- Called synchronously on hover (directory sidecar items are already materialized; I/O is bounded to local SSD reads of small files).
- Returns entries sorted by `updated_at` descending.
- Filters by `query` against `title` (case-insensitive substring match).
- Returns at most `limit` (default 10) entries.
- Returns empty `Vec` on I/O error (silent degradation — no UI for read failures).

**Claude Code implementation:**
1. Derive project dir: `~/.claude/projects/` + path-slug of `directory`.
2. Glob `*.jsonl`, skip files < 100 bytes (empty sessions).
3. For each file, read lines to find `type=ai-title` (title) and first `type=user` (timestamp). Stop reading after both are found.
4. Build `AgentSessionEntry`, collect, sort, filter, truncate.

**Codex implementation:**
1. Open `~/.codex/state_5.sqlite` read-only.
2. `SELECT id, first_user_message, updated_at FROM threads WHERE cwd = ? ORDER BY updated_at DESC`.
3. Filter by `query`, take `limit`.

---

## Changes to `CLIAgent`

Add method:
```rust
pub fn resume_command(&self, session_id: &str) -> String {
    match self {
        CLIAgent::Claude => format!("claude --resume {session_id}"),
        CLIAgent::Codex  => format!("codex --resume {session_id}"),
        _ => unreachable!("resume only for Claude and Codex"),
    }
}

/// Whether this agent supports session resume.
pub fn supports_resume(&self) -> bool {
    matches!(self, CLIAgent::Claude | CLIAgent::Codex)
}
```

---

## Changes to `Workspace` State

```rust
// New fields
sessions_sub_sidecar_menu: ViewHandle<Menu<SessionSidecarSelection>>,
show_sessions_sub_sidecar: bool,
sessions_sub_sidecar_agent: Option<CLIAgent>,
sessions_sub_sidecar_directory: Option<PathBuf>,
sessions_sub_sidecar_filter_editor: ViewHandle<Editor>,
sessions_sub_sidecar_filter: String,
```

`sessions_sub_sidecar_menu` is created in `Workspace::new` like the existing `new_session_sidecar_menu`.

---

## Changes to `build_worktree_sidecar_items()`

When `cli_agent` is `Some(agent)` **and** `agent.supports_resume()`:

- Directory items use `MenuItemFields::new_submenu(path_str)` instead of plain fields.
- `with_on_select_action` is set to `NewSessionSidecarSelection::LaunchCLIAgentInDirectory` (unchanged — click still starts new session).
- The `›` arrow signals a hover-triggered sub-sidecar.

When `cli_agent` is `None` or agent doesn't support resume: unchanged behaviour.

---

## New: `configure_sessions_sub_sidecar(agent, directory, ctx)`

Called when a directory item in sidecar-2 is hovered.

Steps:
1. Set `sessions_sub_sidecar_agent = Some(agent)`, `sessions_sub_sidecar_directory = Some(dir.clone())`.
2. Read sessions: `agent_session_reader::read_sessions(agent, &dir, &self.sessions_sub_sidecar_filter, 10)`.
3. Build `Vec<MenuItem<SessionSidecarSelection>>`:
   - Item 0: search filter editor (same pattern as worktree sidecar).
   - Item 1: `▶ New session` → `SessionSidecarSelection::NewSession { agent, directory }`.
   - Separator.
   - Items 2…N: one per `AgentSessionEntry` → `SessionSidecarSelection::ResumeSession { agent, directory, session_id }`. Label: `"<date>  <title>"` e.g. `"13 май 14:15  Find Codex and Claude chats"`.
   - If no sessions: single disabled item `"No previous sessions"`.
4. Update `sessions_sub_sidecar_menu` with new items.
5. Set `show_sessions_sub_sidecar = true`.

---

## Hover Event Handling

**In `handle_new_session_sidecar_event`** (already handles events from `new_session_sidecar_menu`):

On `MenuEvent::HoveredIndexChanged`:
- Determine hovered item's action.
- If action is `LaunchCLIAgentInDirectory { agent, directory }` and `agent.supports_resume()`:
  - Call `configure_sessions_sub_sidecar(agent, directory, ctx)`.
- Else:
  - Set `show_sessions_sub_sidecar = false`.

---

## Execution: `execute_session_sidecar_selection()`

```rust
fn execute_session_sidecar_selection(
    &mut self,
    selection: SessionSidecarSelection,
    ctx: &mut ViewContext<Self>,
) {
    match selection {
        SessionSidecarSelection::NewSession { agent, directory } => {
            self.launch_cli_agent_in_directory(agent, directory, ctx);
        }
        SessionSidecarSelection::ResumeSession { agent, directory, session_id } => {
            self.launch_cli_agent_with_resume(agent, directory, session_id, ctx);
        }
    }
}
```

`launch_cli_agent_with_resume` is identical to `launch_cli_agent_in_directory` but passes `agent.resume_command(&session_id)` to `execute_command_or_set_pending` instead of `agent.command_prefix()`.

---

## Rendering (Level 3 sidecar)

In the main render method, after the existing `show_new_session_sidecar` block:

```rust
if self.show_sessions_sub_sidecar {
    // anchor_label = path_str of the hovered directory in the L2 sidecar menu
    // (the same string used as MenuItem label in build_worktree_sidecar_items)
    if let Some(anchor_label) = self.sessions_sub_sidecar_directory
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
    {
        let sidecar_element = SavePosition::new(
            ChildView::new(&self.sessions_sub_sidecar_menu).finish(),
            SESSIONS_SUB_SIDECAR_POSITION_ID,
        ).finish();

        let render_left = self.should_render_sidecar_left(
            &anchor_label,
            SESSIONS_SUB_SIDECAR_WIDTH,  // 320px
            app,
        );
        // anchor to hovered dir item in L2 (same offset logic as L1→L2)
        stack.add_positioned_overlay_child(sidecar_element, ...);
    }
}
```

`SESSIONS_SUB_SIDECAR_WIDTH = 320.` (wider than directory sidecar to fit titles).

---

## Filter Behaviour

- Filter editor sits above "New session" in the sub-sidecar.
- On keystroke: re-read sessions with updated query, rebuild items in-place.
- Filter matches against `title` substring (case-insensitive).
- "New session" item is always visible, never filtered.
- Results cap: 10 matching entries.

---

## Error / Edge Cases

| Scenario | Behaviour |
|---|---|
| No sessions found for dir | Single disabled label "No previous sessions" |
| SQLite missing / unreadable | Empty session list (silent) |
| JSONL directory missing for Claude | Empty session list (silent) |
| Session file has no `ai-title` | Use first user prompt[:80] |
| Session file has no user prompt | Skip entry |
| `session_id` contains shell-special chars | Validated: UUIDs only contain `[0-9a-f-]` — safe |
| Agent doesn't support resume | Directory items remain plain (no `›`) |

---

## Out of Scope

- Pagination / "load more" (filter covers this use case).
- Deleting sessions from the UI.
- Showing session length or token count.
- Resume for agents other than Claude Code and Codex.
- Remote sessions.
