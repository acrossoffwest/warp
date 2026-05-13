# CLI Agent Session Resume Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a third-level sessions sub-sidecar to the `+` menu so users can resume previous Codex or Claude Code sessions for a given directory, not just start new ones.

**Architecture:** Extend the existing two-level sidecar system (L1: agent picker, L2: directory picker) with a new L3 sessions sidecar that appears on hover over any directory item when a resume-capable agent is active. A new `agent_session_reader` module reads session metadata from disk (Claude Code JSONL, Codex SQLite) and returns typed `AgentSessionEntry` values. Workspace state tracks the active L3 sidecar; rendering anchors it to the hovered directory label using the existing `offset_from_save_position_element` system.

**Tech Stack:** Rust, GPUI (Warp's UI framework), diesel/SQLite (for Codex sessions), serde_json (for JSONL parsing) — all already in `app/Cargo.toml`.

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `app/src/workspace/agent_session_reader.rs` | **Create** | Read + filter session metadata for a given agent + directory |
| `app/src/workspace/mod.rs` | **Modify** | Declare `pub mod agent_session_reader` |
| `app/src/terminal/cli_agent.rs` | **Modify** | Add `supports_resume()` and `resume_command()` |
| `app/src/workspace/view.rs` | **Modify** | New enum, new state fields, new builder, new hover logic, new render block |

---

## Task 1: `agent_session_reader` module

**Files:**
- Create: `app/src/workspace/agent_session_reader.rs`
- Modify: `app/src/workspace/mod.rs` (add `pub mod agent_session_reader;`)

### What this does

Provides `read_sessions(agent, directory, query, limit) -> Vec<AgentSessionEntry>`.

- **Claude Code:** derive the projects dir slug from `directory` path, glob `~/.claude/projects/<slug>/*.jsonl`, scan lines for `type=ai-title` (title) and first `type=user` (timestamp). Skip files under 100 bytes.
- **Codex:** open `~/.codex/state_5.sqlite` read-only via diesel, run `SELECT id, first_user_message, updated_at FROM threads WHERE cwd = ?`.
- Both: sort by `updated_at` desc, filter by `query` (case-insensitive title substring), return at most `limit` entries.
- I/O errors → empty Vec (silent).

---

- [ ] **Step 1.1: Write tests first**

Create `app/src/workspace/agent_session_reader.rs` with the test module at the bottom. These tests use real local files (Claude Code sessions exist on disk at `~/.claude/projects/`), so they are `#[ignore]` by default — run manually with `-- --ignored`.

```rust
// app/src/workspace/agent_session_reader.rs

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionEntry {
    pub session_id: String,
    /// ai-title (Claude) or first_user_message[:80] (Codex).
    pub title: String,
    /// Unix timestamp seconds (mtime fallback if parse fails).
    pub updated_at: i64,
}

/// Read recent sessions for `agent` in `directory`.
///
/// `query` filters by title (case-insensitive substring). Empty = no filter.
/// `limit` caps results after filtering. Sorted newest-first.
pub fn read_sessions(
    agent: crate::terminal::CLIAgent,
    directory: &Path,
    query: &str,
    limit: usize,
) -> Vec<AgentSessionEntry> {
    match agent {
        crate::terminal::CLIAgent::Claude => read_claude_sessions(directory, query, limit),
        crate::terminal::CLIAgent::Codex => read_codex_sessions(directory, query, limit),
        _ => vec![],
    }
}

// ── Claude Code ──────────────────────────────────────────────────────────────

fn claude_projects_dir() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("CLAUDE_HOME") {
        return Some(PathBuf::from(home).join("projects"));
    }
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

/// Converts an absolute directory path to the Claude Code project slug.
/// `/Users/alice/projects/warp` → `-Users-alice-projects-warp`
fn claude_project_slug(directory: &Path) -> String {
    directory.to_string_lossy().replace('/', "-")
}

fn read_claude_sessions(directory: &Path, query: &str, limit: usize) -> Vec<AgentSessionEntry> {
    let Some(projects_dir) = claude_projects_dir() else {
        return vec![];
    };
    let project_dir = projects_dir.join(claude_project_slug(directory));
    let Ok(entries) = std::fs::read_dir(&project_dir) else {
        return vec![];
    };

    let query_lower = query.to_lowercase();
    let mut sessions: Vec<AgentSessionEntry> = entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .filter(|e| e.metadata().map(|m| m.len() > 100).unwrap_or(false))
        .filter_map(|e| parse_claude_session(&e.path()))
        .filter(|s| {
            query_lower.is_empty()
                || s.title.to_lowercase().contains(query_lower.as_str())
        })
        .collect();

    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions.truncate(limit);
    sessions
}

fn parse_claude_session(path: &Path) -> Option<AgentSessionEntry> {
    let session_id = path.file_stem()?.to_string_lossy().into_owned();
    let content = std::fs::read_to_string(path).ok()?;

    let mut title: Option<String> = None;
    let mut updated_at: Option<i64> = None;

    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("ai-title") if title.is_none() => {
                title = v.get("aiTitle").and_then(|t| t.as_str()).map(str::to_owned);
            }
            Some("user") if updated_at.is_none() => {
                if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                    updated_at = chrono::DateTime::parse_from_rfc3339(ts)
                        .ok()
                        .map(|dt| dt.timestamp());
                }
                // Use as title fallback if no ai-title yet.
                if title.is_none() {
                    title = extract_user_text(&v).map(|t| truncate(&t, 80));
                }
            }
            _ => {}
        }
        if title.is_some() && updated_at.is_some() {
            break;
        }
    }

    // Fallback: mtime.
    let updated_at = updated_at.unwrap_or_else(|| {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64
            })
            .unwrap_or(0)
    });
    let title = title.unwrap_or_default();
    if title.is_empty() {
        return None;
    }

    Some(AgentSessionEntry { session_id, title, updated_at })
}

fn extract_user_text(v: &serde_json::Value) -> Option<String> {
    let content = v.get("message")?.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_owned());
    }
    if let Some(arr) = content.as_array() {
        for item in arr {
            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(s) = item.get("text").and_then(|t| t.as_str()) {
                    return Some(s.to_owned());
                }
            }
        }
    }
    None
}

fn truncate(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

// ── Codex ────────────────────────────────────────────────────────────────────

fn codex_db_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codex").join("state_5.sqlite"))
}

#[derive(diesel::QueryableByName, Debug)]
struct CodexThread {
    #[diesel(sql_type = diesel::sql_types::Text)]
    id: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    first_user_message: Option<String>,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    updated_at: i64,
}

fn read_codex_sessions(directory: &Path, query: &str, limit: usize) -> Vec<AgentSessionEntry> {
    use diesel::prelude::*;
    use diesel::sqlite::SqliteConnection;

    let Some(db_path) = codex_db_path() else {
        return vec![];
    };
    let Ok(db_str) = db_path.to_str().map(str::to_owned).ok_or(()) else {
        return vec![];
    };
    let Ok(mut conn) = SqliteConnection::establish(&db_str) else {
        return vec![];
    };

    let cwd = directory.to_string_lossy().into_owned();
    let query_lower = query.to_lowercase();

    let Ok(rows) = diesel::sql_query(
        "SELECT id, first_user_message, updated_at FROM threads \
         WHERE cwd = ? ORDER BY updated_at DESC",
    )
    .bind::<diesel::sql_types::Text, _>(&cwd)
    .load::<CodexThread>(&mut conn) else {
        return vec![];
    };

    rows.into_iter()
        .filter_map(|row| {
            let title = truncate(row.first_user_message?.trim(), 80);
            if title.is_empty() {
                return None;
            }
            if !query_lower.is_empty()
                && !title.to_lowercase().contains(query_lower.as_str())
            {
                return None;
            }
            Some(AgentSessionEntry {
                session_id: row.id,
                title,
                updated_at: row.updated_at,
            })
        })
        .take(limit)
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn warp_dir() -> PathBuf {
        PathBuf::from("/Users/[redacted]/projects/own-projects/warp")
    }

    #[test]
    #[ignore = "reads real ~/.claude/projects — run manually"]
    fn claude_sessions_found_for_warp() {
        let sessions = read_claude_sessions(&warp_dir(), "", 10);
        assert!(
            !sessions.is_empty(),
            "expected at least one Claude session for warp dir"
        );
        let first = &sessions[0];
        assert!(!first.session_id.is_empty());
        assert!(!first.title.is_empty());
        assert!(first.updated_at > 0);
        // Sorted newest first.
        if sessions.len() > 1 {
            assert!(sessions[0].updated_at >= sessions[1].updated_at);
        }
    }

    #[test]
    #[ignore = "reads real ~/.claude/projects — run manually"]
    fn claude_sessions_filter_works() {
        let all = read_claude_sessions(&warp_dir(), "", 10);
        let filtered = read_claude_sessions(&warp_dir(), "codex", 10);
        // Filtered set is a subset.
        assert!(filtered.len() <= all.len());
        for s in &filtered {
            assert!(s.title.to_lowercase().contains("codex"));
        }
    }

    #[test]
    #[ignore = "reads real ~/.codex/state_5.sqlite — run manually"]
    fn codex_sessions_found_for_warp() {
        let sessions = read_codex_sessions(&warp_dir(), "", 10);
        assert!(
            !sessions.is_empty(),
            "expected Codex sessions for warp dir"
        );
        let first = &sessions[0];
        assert!(!first.session_id.is_empty());
        assert!(!first.title.is_empty());
    }

    #[test]
    fn claude_slug_derivation() {
        let path = PathBuf::from("/Users/alice/projects/warp");
        assert_eq!(claude_project_slug(&path), "-Users-alice-projects-warp");
    }

    #[test]
    fn truncate_works() {
        assert_eq!(truncate("hello world", 5), "hello…");
        assert_eq!(truncate("hi", 5), "hi");
        assert_eq!(truncate("", 5), "");
    }

    #[test]
    fn empty_vec_for_unknown_agent() {
        let dir = PathBuf::from("/tmp");
        let result = read_sessions(crate::terminal::CLIAgent::Gemini, &dir, "", 10);
        assert!(result.is_empty());
    }
}
```

- [ ] **Step 1.2: Register module in `app/src/workspace/mod.rs`**

Open `app/src/workspace/mod.rs`. After the last `pub mod` line (currently `pub mod view;`), add:

```rust
pub mod agent_session_reader;
```

- [ ] **Step 1.3: Run compile check**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
cargo check -p warp 2>&1 | tail -30
```

Expected: compiles clean (zero errors). Warnings about unused functions are acceptable at this stage.

- [ ] **Step 1.4: Run unit tests (non-ignored)**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
cargo test -p warp agent_session_reader 2>&1 | tail -20
```

Expected: `claude_slug_derivation`, `truncate_works`, `empty_vec_for_unknown_agent` — all PASS.

- [ ] **Step 1.5: Run ignored integration tests manually**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
cargo test -p warp agent_session_reader -- --ignored 2>&1 | tail -30
```

Expected: all four ignored tests PASS (they read real local files; if the session files exist, they should pass).

- [ ] **Step 1.6: Commit**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
git add app/src/workspace/agent_session_reader.rs app/src/workspace/mod.rs
git commit -m "feat(workspace): add agent_session_reader for Claude/Codex session metadata"
```

---

## Task 2: `CLIAgent` extensions

**Files:**
- Modify: `app/src/terminal/cli_agent.rs`

Add two methods to the `CLIAgent` impl block.

---

- [ ] **Step 2.1: Add tests in `app/src/terminal/cli_agent_tests.rs`**

Open `app/src/terminal/cli_agent_tests.rs`. Append inside the existing test module:

```rust
#[test]
fn supports_resume_only_for_claude_and_codex() {
    assert!(CLIAgent::Claude.supports_resume());
    assert!(CLIAgent::Codex.supports_resume());
    assert!(!CLIAgent::Gemini.supports_resume());
    assert!(!CLIAgent::Amp.supports_resume());
}

#[test]
fn resume_command_format() {
    let id = "019e159b-717d-7663-9a93-95fd9c0790b1";
    assert_eq!(
        CLIAgent::Claude.resume_command(id),
        format!("claude --resume {id}")
    );
    assert_eq!(
        CLIAgent::Codex.resume_command(id),
        format!("codex --resume {id}")
    );
}
```

- [ ] **Step 2.2: Run to confirm failure**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
cargo test -p warp cli_agent_tests 2>&1 | grep -E "FAILED|error"
```

Expected: compile error — `supports_resume` and `resume_command` don't exist yet.

- [ ] **Step 2.3: Add methods to `CLIAgent`**

Open `app/src/terminal/cli_agent.rs`. Find the `impl CLIAgent` block (around the `command_prefix` method). Add after the last existing method in that impl block:

```rust
/// Whether this agent supports `--resume <session-id>`.
pub fn supports_resume(&self) -> bool {
    matches!(self, CLIAgent::Claude | CLIAgent::Codex)
}

/// Shell command to resume an existing session by ID.
///
/// Callers must ensure `session_id` is a UUID (contains only `[0-9a-f-]`).
pub fn resume_command(&self, session_id: &str) -> String {
    match self {
        CLIAgent::Claude => format!("claude --resume {session_id}"),
        CLIAgent::Codex => format!("codex --resume {session_id}"),
        other => {
            log::warn!("resume_command called on non-resumable agent {other:?}");
            self.command_prefix().to_owned()
        }
    }
}
```

- [ ] **Step 2.4: Run tests to confirm pass**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
cargo test -p warp cli_agent_tests 2>&1 | tail -10
```

Expected: all tests PASS.

- [ ] **Step 2.5: Commit**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
git add app/src/terminal/cli_agent.rs app/src/terminal/cli_agent_tests.rs
git commit -m "feat(cli_agent): add supports_resume and resume_command"
```

---

## Task 3: Workspace state + `SessionSidecarSelection` enum

**Files:**
- Modify: `app/src/workspace/view.rs`

Add the new enum, new state fields, and init them in `Workspace::new`.

---

- [ ] **Step 3.1: Add `SessionSidecarSelection` enum**

Open `app/src/workspace/view.rs`. Around line 803, after the closing brace of `NewSessionSidecarSelection`, add:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
enum SessionSidecarSelection {
    NewSession { agent: CLIAgent, directory: PathBuf },
    ResumeSession { agent: CLIAgent, directory: PathBuf, session_id: String },
}
```

- [ ] **Step 3.2: Add constants**

Near the other `NEW_SESSION_SIDECAR_*` constants (around line 610), add:

```rust
const SESSIONS_SUB_SIDECAR_POSITION_ID: &str = "sessions_sub_sidecar";
const SESSIONS_SUB_SIDECAR_WIDTH: f32 = 320.;
```

- [ ] **Step 3.3: Add new state fields to `Workspace` struct**

Find the `Workspace` struct field block where `new_session_sidecar_menu`, `show_new_session_sidecar`, `new_session_sidecar_cli_agent` are declared (around line 1072). Directly after those three fields add:

```rust
sessions_sub_sidecar_menu: ViewHandle<Menu<SessionSidecarSelection>>,
show_sessions_sub_sidecar: bool,
sessions_sub_sidecar_agent: Option<CLIAgent>,
sessions_sub_sidecar_directory: Option<PathBuf>,
sessions_sub_sidecar_filter_editor: ViewHandle<EditorView>,
sessions_sub_sidecar_filter: String,
```

- [ ] **Step 3.4: Initialize new fields in `Workspace::new`**

Find where `new_session_sidecar_menu` is created (around line 1842, inside the function that builds the menus and returns the triple). After `new_session_sidecar` is created, add:

```rust
let sessions_sub_sidecar = ctx.add_typed_action_view(|_ctx| {
    Menu::<SessionSidecarSelection>::new(vec![])
        .with_width(SESSIONS_SUB_SIDECAR_WIDTH)
});
ctx.subscribe_to_view(&sessions_sub_sidecar, move |me, _, event, ctx| {
    me.handle_sessions_sub_sidecar_event(event, ctx);
});
```

Then in the struct literal inside `Workspace::new` (around line 3226), after `new_session_sidecar_cli_agent: None,` add:

```rust
sessions_sub_sidecar_menu: sessions_sub_sidecar,
show_sessions_sub_sidecar: false,
sessions_sub_sidecar_agent: None,
sessions_sub_sidecar_directory: None,
sessions_sub_sidecar_filter_editor: Self::build_sessions_sub_sidecar_filter_input(ctx),
sessions_sub_sidecar_filter: String::new(),
```

- [ ] **Step 3.5: Compile check**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
cargo check -p warp 2>&1 | grep "^error" | head -20
```

Expected: errors for missing `handle_sessions_sub_sidecar_event` and `build_sessions_sub_sidecar_filter_input` — which are added in Task 4. Other errors indicate a mistake in Task 3 and must be fixed before proceeding.

- [ ] **Step 3.6: Commit (partial compile)**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
git add app/src/workspace/view.rs
git commit -m "feat(workspace): add SessionSidecarSelection + sub-sidecar state fields"
```

---

## Task 4: Sessions sub-sidecar builder + `configure_sessions_sub_sidecar`

**Files:**
- Modify: `app/src/workspace/view.rs`

Add the filter editor builder, the configure function, and the event handler for the sub-sidecar menu.

---

- [ ] **Step 4.1: Add `build_sessions_sub_sidecar_filter_input`**

Find `build_worktree_sidecar_search_input` (around line 1231). Directly after its closing brace, add the following function. It follows the exact same pattern but navigates the *sessions* sub-sidecar:

```rust
fn build_sessions_sub_sidecar_filter_input(
    ctx: &mut ViewContext<Self>,
) -> ViewHandle<EditorView> {
    let editor = ctx.add_typed_action_view(|ctx| {
        let appearance = Appearance::as_ref(ctx);
        let mut editor = EditorView::single_line(
            SingleLineEditorOptions {
                text: TextOptions::ui_text(Some(appearance.ui_font_size()), appearance),
                select_all_on_focus: true,
                clear_selections_on_blur: true,
                propagate_and_no_op_vertical_navigation_keys:
                    PropagateAndNoOpNavigationKeys::Always,
                ..Default::default()
            },
            ctx,
        );
        editor.set_placeholder_text("Filter sessions…", ctx);
        editor
    });
    ctx.subscribe_to_view(&editor, |me, editor_view, event, ctx| match event {
        EditorEvent::Edited(_) => {
            me.sessions_sub_sidecar_filter =
                editor_view.as_ref(ctx).buffer_text(ctx);
            me.refresh_sessions_sub_sidecar_if_active(ctx);
            ctx.notify();
        }
        EditorEvent::Escape => {
            me.close_new_session_dropdown_menu(ctx);
        }
        EditorEvent::Navigate(NavigationKey::Up) => {
            me.sessions_sub_sidecar_menu.update(ctx, |menu, view_ctx| {
                menu.select_previous(view_ctx);
            });
        }
        EditorEvent::Navigate(NavigationKey::Down) => {
            me.sessions_sub_sidecar_menu.update(ctx, |menu, view_ctx| {
                menu.select_next(view_ctx);
            });
        }
        EditorEvent::Enter => {
            if let Some(sel) = me
                .sessions_sub_sidecar_menu
                .read(ctx, |menu, _| menu.selected_item_action().cloned())
            {
                me.execute_session_sidecar_selection(sel, ctx);
                me.close_new_session_dropdown_menu(ctx);
            }
        }
        _ => {}
    });
    editor
}
```

- [ ] **Step 4.2: Add `refresh_sessions_sub_sidecar_if_active`**

Directly after `build_sessions_sub_sidecar_filter_input`, add:

```rust
fn refresh_sessions_sub_sidecar_if_active(&mut self, ctx: &mut ViewContext<Self>) {
    if !self.show_sessions_sub_sidecar {
        return;
    }
    let Some(agent) = self.sessions_sub_sidecar_agent else { return };
    let Some(directory) = self.sessions_sub_sidecar_directory.clone() else { return };
    self.configure_sessions_sub_sidecar(agent, directory, ctx);
}
```

- [ ] **Step 4.3: Add `configure_sessions_sub_sidecar`**

Find `configure_worktree_new_session_sidecar` (around line 8775). Directly after it, add:

```rust
fn configure_sessions_sub_sidecar(
    &mut self,
    agent: CLIAgent,
    directory: PathBuf,
    ctx: &mut ViewContext<Self>,
) {
    use crate::workspace::agent_session_reader;

    self.sessions_sub_sidecar_agent = Some(agent);
    self.sessions_sub_sidecar_directory = Some(directory.clone());

    let sessions = agent_session_reader::read_sessions(
        agent,
        &directory,
        &self.sessions_sub_sidecar_filter,
        10,
    );

    let filter_editor = self.sessions_sub_sidecar_filter_editor.clone();
    let filter_item = MenuItemFields::new_with_custom_label(
        Arc::new(move |_, _, appearance, _| {
            let theme = appearance.theme();
            let search_icon = ConstrainedBox::new(
                icons::Icon::SearchSmall
                    .to_warpui_icon(theme.sub_text_color(theme.surface_2()))
                    .finish(),
            )
            .with_width(16.)
            .with_height(16.)
            .finish();
            let row = Flex::row()
                .with_child(Container::new(search_icon).with_margin_right(8.).finish())
                .with_child(
                    Shrinkable::new(1., ChildView::new(&filter_editor).finish()).finish(),
                )
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Max)
                .finish();
            ConstrainedBox::new(
                Container::new(row)
                    .with_padding_left(NEW_SESSION_SIDECAR_SEARCH_BOX_HORIZONTAL_PADDING)
                    .with_padding_right(NEW_SESSION_SIDECAR_SEARCH_BOX_HORIZONTAL_PADDING)
                    .with_padding_top(NEW_SESSION_SIDECAR_SEARCH_BOX_VERTICAL_PADDING)
                    .with_padding_bottom(NEW_SESSION_SIDECAR_SEARCH_BOX_VERTICAL_PADDING)
                    .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
                    .with_corner_radius(CornerRadius::with_top(Radius::Pixels(4.)))
                    .finish(),
            )
            .with_height(NEW_SESSION_SIDECAR_SEARCH_BOX_HEIGHT)
            .finish()
        }),
        Some("Filter sessions".to_string()),
    )
    .with_no_interaction_on_hover()
    .no_highlight_on_hover()
    .with_padding_override(0., 0.)
    .into_item();

    let new_session_item = MenuItemFields::new("▶ New session")
        .with_on_select_action(SessionSidecarSelection::NewSession {
            agent,
            directory: directory.clone(),
        })
        .into_item();

    let mut items = vec![filter_item, new_session_item, MenuItem::Separator];

    if sessions.is_empty() {
        items.push(
            MenuItemFields::new("No previous sessions")
                .with_disabled(true)
                .into_item(),
        );
    } else {
        for session in sessions {
            let label = format_session_label(session.updated_at, &session.title);
            items.push(
                MenuItemFields::new(label)
                    .with_on_select_action(SessionSidecarSelection::ResumeSession {
                        agent,
                        directory: directory.clone(),
                        session_id: session.session_id,
                    })
                    .into_item(),
            );
        }
    }

    self.sessions_sub_sidecar_menu
        .update(ctx, |menu, view_ctx| menu.set_items(items, view_ctx));
    self.show_sessions_sub_sidecar = true;
    ctx.notify();
}

fn hide_sessions_sub_sidecar(&mut self, ctx: &mut ViewContext<Self>) {
    if self.show_sessions_sub_sidecar {
        self.show_sessions_sub_sidecar = false;
        self.sessions_sub_sidecar_agent = None;
        self.sessions_sub_sidecar_directory = None;
        ctx.notify();
    }
}
```

- [ ] **Step 4.4: Add `format_session_label` free function**

Near the top of the file (or just before `configure_sessions_sub_sidecar`), add:

```rust
fn format_session_label(updated_at: i64, title: &str) -> String {
    use chrono::{DateTime, Local, Utc};
    let dt: DateTime<Local> = DateTime::<Utc>::from_timestamp(updated_at, 0)
        .unwrap_or_default()
        .into();
    format!("{}  {}", dt.format("%d %b %H:%M"), title)
}
```

- [ ] **Step 4.5: Add `handle_sessions_sub_sidecar_event` and `execute_session_sidecar_selection`**

Directly after `handle_new_session_sidecar_event` (around line 8672), add:

```rust
fn handle_sessions_sub_sidecar_event(
    &mut self,
    event: &MenuEvent,
    ctx: &mut ViewContext<Self>,
) {
    match event {
        MenuEvent::Close { via_select_item } => {
            if *via_select_item {
                let sel = self
                    .sessions_sub_sidecar_menu
                    .read(ctx, |menu, _| menu.selected_item_action().cloned());
                if let Some(sel) = sel {
                    self.execute_session_sidecar_selection(sel, ctx);
                    self.show_new_session_dropdown_menu = None;
                }
            }
            self.hide_sessions_sub_sidecar(ctx);
            ctx.notify();
        }
        MenuEvent::ItemSelected => {}
        MenuEvent::ItemHovered => {}
    }
}

fn execute_session_sidecar_selection(
    &mut self,
    selection: SessionSidecarSelection,
    ctx: &mut ViewContext<Self>,
) {
    match selection {
        SessionSidecarSelection::NewSession { agent, directory } => {
            self.launch_cli_agent_in_directory(agent, directory, ctx);
        }
        SessionSidecarSelection::ResumeSession {
            agent,
            directory,
            session_id,
        } => {
            self.launch_cli_agent_with_resume(agent, directory, session_id, ctx);
        }
    }
}
```

- [ ] **Step 4.6: Add `launch_cli_agent_with_resume`**

Find `launch_cli_agent_in_directory` (around line 10801). Directly after it, add:

```rust
fn launch_cli_agent_with_resume(
    &mut self,
    agent: CLIAgent,
    directory: PathBuf,
    session_id: String,
    ctx: &mut ViewContext<Self>,
) {
    self.add_tab_with_pane_layout(
        PanesLayout::SingleTerminal(Box::new(NewTerminalOptions {
            initial_directory: Some(directory),
            hide_homepage: true,
            ..Default::default()
        })),
        Arc::new(HashMap::new()),
        None,
        ctx,
    );

    let Some(terminal_view) = self
        .active_tab_pane_group()
        .as_ref(ctx)
        .active_session_view(ctx)
    else {
        log::warn!(
            "Could not find terminal after creating tab for {} resume",
            agent.display_name()
        );
        return;
    };

    terminal_view.update(ctx, |terminal_view, ctx| {
        terminal_view.execute_command_or_set_pending(
            agent.resume_command(&session_id),
            ctx,
        );
    });
    ctx.notify();
}
```

- [ ] **Step 4.7: Compile check**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
cargo check -p warp 2>&1 | grep "^error" | head -30
```

Expected: only errors related to `select_previous`, `select_next`, `selected_item_action` (Menu API names to verify — adjust if the actual method names differ). To find correct method names:

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
grep -rn "fn select_prev\|fn select_next\|fn selected_item\|fn hovered_item" crates/ --include="*.rs" | head -20
```

Adjust calls to match actual API names in `Menu<T>`.

- [ ] **Step 4.8: Commit**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
git add app/src/workspace/view.rs
git commit -m "feat(workspace): add sessions sub-sidecar builder and execution"
```

---

## Task 5: Hover wiring + directory items as sub-menu

**Files:**
- Modify: `app/src/workspace/view.rs`

Two changes: (1) directory items in L2 become `new_submenu` when agent supports resume, (2) hovering a directory triggers/hides the L3 sidecar.

---

- [ ] **Step 5.1: Extend `build_worktree_sidecar_items()` to use `new_submenu` for directories**

Find `build_worktree_sidecar_items` (around line 8690). Inside the `.map(|ws| { ... })` closure, change the branch `if let Some(agent) = cli_agent { fields.with_on_select_action(...) }` to:

```rust
fields = if let Some(agent) = cli_agent {
    let base = fields.with_on_select_action(
        NewSessionSidecarSelection::LaunchCLIAgentInDirectory {
            agent,
            directory: ws.path.clone(),
        },
    );
    if agent.supports_resume() {
        // Show › arrow. Click still fires on_select_action (new session).
        // Hover triggers L3 sessions sidecar (wired in handle_new_session_sidecar_event).
        base.into_submenu_fields()
    } else {
        base
    }
} else {
    fields.with_on_select_action(NewSessionSidecarSelection::OpenWorktreeRepo {
        repo_path: path_str,
    })
};
```

> **Note:** `into_submenu_fields()` is the builder method that adds `›` without changing the select action. Verify the actual method name:
> ```bash
> grep -n "fn.*submenu\|new_submenu\|into_submenu" crates/warpui/src/ -r --include="*.rs" | head -20
> ```
> Use the matching method name. The spec says `MenuItemFields::new_submenu(label)` creates a submenu-labelled item from scratch. Here we need to convert an existing `fields` to a submenu-styled item. If no conversion method exists, use `MenuItemFields::new_submenu(path_str.clone()).with_on_select_action(...)` instead.

- [ ] **Step 5.2: Wire hover detection in `handle_new_session_sidecar_event`**

Find `handle_new_session_sidecar_event` (around line 8641). Replace the `MenuEvent::ItemHovered` arm:

```rust
MenuEvent::ItemHovered => {
    self.sync_new_session_sidecar_selection_to_hover(ctx);
    self.sync_sessions_sub_sidecar_to_sidecar_hover(ctx);
}
```

Then add the new helper after `sync_new_session_sidecar_selection_to_hover`:

```rust
fn sync_sessions_sub_sidecar_to_sidecar_hover(&mut self, ctx: &mut ViewContext<Self>) {
    let hovered = self.new_session_sidecar_menu.read(ctx, |menu, _| {
        let idx = menu.hovered_index()?;
        let item = menu.items().get(idx)?;
        MenuItem::item_on_select_action(item).cloned()
    });

    match hovered {
        Some(NewSessionSidecarSelection::LaunchCLIAgentInDirectory { agent, directory })
            if agent.supports_resume() =>
        {
            let needs_configure = self.sessions_sub_sidecar_directory.as_ref()
                != Some(&directory)
                || self.sessions_sub_sidecar_agent != Some(agent);
            if needs_configure {
                self.configure_sessions_sub_sidecar(agent, directory, ctx);
            }
        }
        _ => {
            self.hide_sessions_sub_sidecar(ctx);
        }
    }
}
```

- [ ] **Step 5.3: Reset sub-sidecar when L2 sidecar closes**

Find `clear_worktree_sidecar_state` (search for that function name). At the end of it, add:

```rust
self.hide_sessions_sub_sidecar(ctx);
```

- [ ] **Step 5.4: Compile check**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
cargo check -p warp 2>&1 | grep "^error" | head -20
```

Resolve any errors (likely the `into_submenu_fields()` API name, or borrow checker issues with `directory` moved into the configure call). Fix before continuing.

- [ ] **Step 5.5: Commit**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
git add app/src/workspace/view.rs
git commit -m "feat(workspace): wire directory hover to sessions sub-sidecar"
```

---

## Task 6: Render L3 sidecar + final compile + manual smoke test

**Files:**
- Modify: `app/src/workspace/view.rs` (render method)

---

- [ ] **Step 6.1: Add L3 render block**

Find the render method section that shows the L2 sidecar (the `if self.show_new_session_sidecar` block ending around line 23208). Directly after that block's closing brace, add:

```rust
// Level 3 — sessions sub-sidecar (anchored to hovered directory in L2).
if self.show_sessions_sub_sidecar {
    if let Some(anchor_label) = self
        .sessions_sub_sidecar_directory
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
    {
        let sidecar_element = SavePosition::new(
            ChildView::new(&self.sessions_sub_sidecar_menu).finish(),
            SESSIONS_SUB_SIDECAR_POSITION_ID,
        )
        .finish();

        let render_left = self.should_render_sidecar_left(
            &anchor_label,
            SESSIONS_SUB_SIDECAR_WIDTH,
            app,
        );
        let (offset, parent_anchor, child_anchor) = if render_left {
            (
                vec2f(-4., 0.),
                PositionedElementAnchor::TopLeft,
                ChildAnchor::TopRight,
            )
        } else {
            (
                vec2f(4., 0.),
                PositionedElementAnchor::TopRight,
                ChildAnchor::TopLeft,
            )
        };

        stack.add_positioned_overlay_child(
            sidecar_element,
            OffsetPositioning::offset_from_save_position_element(
                anchor_label,
                offset,
                PositionedElementOffsetBounds::WindowByPosition,
                parent_anchor,
                child_anchor,
            ),
        );
    }
}
```

- [ ] **Step 6.2: Full compile check**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
cargo check -p warp 2>&1 | tail -20
```

Expected: zero errors. Warnings acceptable.

- [ ] **Step 6.3: Run all relevant tests**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
cargo test -p warp agent_session_reader cli_agent_tests 2>&1 | tail -20
```

Expected: all non-ignored tests PASS.

- [ ] **Step 6.4: Build and manual smoke test**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
cargo build -p warp 2>&1 | tail -10
```

Then launch Warp from the worktree build, open the `+` menu, hover over Codex or Claude Code, then hover over `~/projects/own-projects/warp`. Verify:

1. L2 directory sidecar appears with `›` arrows on each directory row.
2. Hovering `~/projects/own-projects/warp` opens L3 sub-sidecar.
3. L3 shows filter editor + "▶ New session" + previous sessions with dates and titles.
4. Clicking "▶ New session" opens a new terminal tab and runs `claude` / `codex`.
5. Clicking a session item opens a new terminal tab and runs `claude --resume <id>` / `codex --resume <id>`.
6. Typing in the filter narrows the session list.
7. Clicking the directory directly (without hovering to L3) also opens a new session.

- [ ] **Step 6.5: Commit**

```bash
cd /Users/[redacted]/projects/own-projects/warp-worktrees/feat-remote-control
git add app/src/workspace/view.rs
git commit -m "feat(workspace): render sessions sub-sidecar for CLI agent session resume"
```

---

## Self-Review

**Spec coverage:**
- ✅ Three-level nav (L1 agent → L2 directory → L3 sessions)
- ✅ Directory click = new session; hover = L3
- ✅ L3 filter editor (filter by title, case-insensitive)
- ✅ "▶ New session" always first in L3
- ✅ Sessions sorted newest-first, limit 10
- ✅ Claude: `ai-title` field, fallback first user prompt
- ✅ Codex: SQLite `threads.first_user_message`
- ✅ `AgentSessionEntry.session_id` → `resume_command()`
- ✅ `launch_cli_agent_with_resume` sends `--resume <id>` to terminal
- ✅ Empty sessions → "No previous sessions" disabled item
- ✅ I/O errors → empty Vec (silent)
- ✅ Agents without `supports_resume()` → directory items unchanged (no `›`)

**Type consistency check:**
- `SessionSidecarSelection` defined Task 3, used in Tasks 4/5 ✅
- `configure_sessions_sub_sidecar(agent: CLIAgent, directory: PathBuf, ctx)` defined Task 4, called Task 5 ✅
- `hide_sessions_sub_sidecar(ctx)` defined Task 4, called Tasks 5/6 ✅
- `agent_session_reader::read_sessions(agent, directory, query, limit)` defined Task 1, called Task 4 ✅
- `AgentSessionEntry { session_id, title, updated_at }` defined Task 1, consumed Task 4 ✅
- `format_session_label(updated_at: i64, title: &str) -> String` defined Task 4, called Task 4 ✅
- `SESSIONS_SUB_SIDECAR_POSITION_ID`, `SESSIONS_SUB_SIDECAR_WIDTH` defined Task 3, used Task 6 ✅

**No placeholders:** All steps have concrete code or explicit lookup instructions. ✅
