use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionEntry {
    pub session_id: String,
    pub title: String,
    pub updated_at: i64,
}

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

fn claude_projects_dir() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("CLAUDE_HOME") {
        return Some(PathBuf::from(home).join("projects"));
    }
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

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
    let db_str = match db_path.to_str() {
        Some(s) => s.to_owned(),
        None => return vec![],
    };
    let Ok(mut conn) = SqliteConnection::establish(&db_str) else {
        return vec![];
    };

    let cwd = directory.to_string_lossy().into_owned();
    let query_lower = query.to_lowercase();

    let Ok(rows) = diesel::sql_query(
        "SELECT id, first_user_message, updated_at FROM threads WHERE cwd = ? ORDER BY updated_at DESC",
    )
    .bind::<diesel::sql_types::Text, _>(&cwd)
    .load::<CodexThread>(&mut conn) else {
        return vec![];
    };

    rows.into_iter()
        .filter_map(|row| {
            let raw = row.first_user_message?;
            let title = truncate(raw.trim(), 80);
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
        if sessions.len() > 1 {
            assert!(sessions[0].updated_at >= sessions[1].updated_at);
        }
    }

    #[test]
    #[ignore = "reads real ~/.claude/projects — run manually"]
    fn claude_sessions_filter_works() {
        let all = read_claude_sessions(&warp_dir(), "", 10);
        let filtered = read_claude_sessions(&warp_dir(), "codex", 10);
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
