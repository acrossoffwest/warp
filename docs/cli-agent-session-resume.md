# CLI Agent Session Resume

Adds a third-level (L3) sub-sidecar to the `+` menu so users can resume previous
Claude Code and Codex sessions for a given directory, plus a per-agent settings
toggle for default "dangerous" launch flags.

## UX

`+` dropdown (L1) → `Claude Code` / `Codex` submenu (L2: directory picker) →
hovering a directory opens an L3 sidecar listing that directory's previous
sessions. Clicking the directory itself starts a new session; clicking a
session row resumes via `--resume <id>`.

L3 has its own filter input plus a `▶ New session` row.

## File layout

- `app/src/workspace/agent_session_reader.rs` — reads session metadata from
  disk for both agents. Public API:
  - `read_sessions(agent, dir, query, limit)` — filtered + truncated.
  - `read_all_sessions(agent, dir)` — unfiltered list (used by the cache).
  - `source_version(agent, dir)` — cheap mtime stat used as cache version.
- `app/src/terminal/cli_agent.rs` — adds `supports_resume()`, `resume_command()`,
  `dangerous_flag()`, `launch_command(dangerous)`, and
  `resume_command_with_flags(id, dangerous)`.
- `app/src/workspace/view.rs` — L3 menu state, hover wiring, render block,
  in-memory `sessions_cache: HashMap<(CLIAgent, PathBuf), (mtime, Vec<entry>)>`,
  and `cli_agent_dangerous_flag_enabled(agent, ctx)` helper that reads the
  per-agent opt-ins from `AISettings`.
- `app/src/settings/ai.rs` — two new bool settings:
  - `claude_dangerously_skip_permissions` (`agents.third_party.claude_dangerously_skip_permissions`)
  - `codex_dangerously_bypass_approvals` (`agents.third_party.codex_dangerously_bypass_approvals_and_sandbox`)
- `app/src/settings_view/ai_page.rs` — extends the existing `Third party CLI
  agents` settings page with a `Default launch flags` subsection containing
  the two toggles. Both toggles are always visible, independent of the
  coding-agent-toolbar setting.

## Claude Code session source

Sessions live in `~/.claude/projects/<slug>/<session-id>.jsonl`. Slug derivation:
replace both `/` and `.` with `-` in the absolute directory path (Claude's own
convention).

Per-session metadata extraction priority for the title shown in the list:

1. `type: "custom-title"` — set by the user via `/rename`. **Last write
   wins** (the file may contain multiple `custom-title` events; the latest
   one is what the user sees in Claude Code's UI).
2. `type: "ai-title"` — AI-generated summary.
3. First user message — only if its text isn't a slash-command system block
   (`<local-command-caveat>`, `Caveat:`, `<command-name>`, `<command-message>`,
   `<command-args>`); residual XML-ish tags are stripped.

`updated_at` comes from the timestamp of the first `user` event, falling back
to file mtime.

## Codex session source

`~/.codex/state_5.sqlite`, table `threads`, columns `id`,
`first_user_message`, `updated_at`. Read read-only via the `file:...?mode=ro`
SQLite URI (works because `libsqlite3-sys` is built with the `bundled`
feature, which enables URI filenames). `updated_at` is in Unix seconds.

## Performance

Two layers:

1. **Bounded file scan** in `parse_claude_session`:
   - Head pass (first 200 lines) collects `ai-title`, first user message, and
     first timestamp; stops early once both title and timestamp are known.
   - Custom-title pass iterates all lines but only JSON-parses lines whose raw
     text contains `"custom-title"`. A substring check is ~100× cheaper than
     `serde_json` for the majority of lines.
2. **In-memory cache** keyed by `(CLIAgent, PathBuf)`:
   - Stores the full unfiltered list plus the source mtime.
   - On hover, `source_version` is re-stated and compared. If unchanged, the
     cached list is reused; filter and `take(25)` are applied in memory.
   - Cache invalidates automatically when the project dir or `state_5.sqlite`
     gets a new mtime (e.g., a new session is written).

## Settings: default launch flags

Two opt-in toggles in `Settings → Third party CLI agents → Default launch flags`:

- **Claude Code: `--dangerously-skip-permissions`** — appends the flag to
  `claude` for both new and resumed sessions started via the `+` menu.
- **Codex: `--dangerously-bypass-approvals-and-sandbox`** — same idea for
  `codex`.

Both default to `false`. The flag is applied in `launch_cli_agent_in_directory`
and `launch_cli_agent_with_resume` via `cli_agent_dangerous_flag_enabled`.
Agents without a dangerous flag (Gemini, Amp, etc.) are unaffected.

## UI polish

- L2 paths under `$HOME` render as `~/...` (`abbreviate_home_path`).
  Falls back to `/Users/$USER` / `/home/$USER` when `HOME` is overridden
  (e.g., during dev), so paths still abbreviate inside an isolated dev run.
- Long paths get `…` prefix truncation at 48 chars so the project name (end
  of the path) stays visible.
- L2 width 420 px, L3 width 480 px (wider than the default sidecar so paths
  and session titles fit without aggressive truncation).
- L3 limit is 25 most-recent sessions (filter is global across all sessions
  in the directory; only the result is capped).

## Menu mechanics

Submenu-parent rows fire `HoverSubmenuWithChildren`, which updates
`selected_index` rather than `hovered_index` (only leaf rows update
`hovered_index`). `sync_sessions_sub_sidecar_to_sidecar_hover` reads
`selected_index().or_else(|| hovered_index())` so both row types work.

`ItemSelected` and `ItemHovered` events on the L2 menu both trigger the L3
sync, ensuring the L3 sub-sidecar opens whether the user reaches the
directory row via the mouse or keyboard.

The L2→L3 safe-zone uses the same `set_safe_zone_target` /
`set_submenu_being_shown_for_item_index` pattern as L1→L2, refreshed on every
hover event so the L3 rect is picked up after the first frame in which the
sidecar renders.
