use std::path::{Path, PathBuf};

use crate::session_memory::restore::AgentSessionFile;
use crate::session_memory::types::is_valid_session_id;
use crate::terminal::CLIAgent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionEntry {
    pub session_id: String,
    pub title: String,
    pub updated_at: i64,
    pub created_at: i64,
    pub cwd: Option<PathBuf>,
    pub transcript_path: Option<PathBuf>,
    pub source: CLIAgent,
    pub launch_argv: Option<Vec<String>>,
}

pub fn read_sessions(
    agent: CLIAgent,
    directory: &Path,
    query: &str,
    limit: usize,
) -> Vec<AgentSessionEntry> {
    let mut all = read_all_sessions(agent, directory);
    let q = query.to_lowercase();
    if !q.is_empty() {
        all.retain(|s| s.title.to_lowercase().contains(q.as_str()));
    }
    all.truncate(limit);
    all
}

/// Returns the unfiltered, unlimited list of sessions for the given (agent,
/// directory), sorted newest-first. Callers that want to cache should pair
/// this with [`source_version`] to detect changes cheaply.
pub fn read_all_sessions(agent: CLIAgent, directory: &Path) -> Vec<AgentSessionEntry> {
    match agent {
        CLIAgent::Claude => read_claude_sessions_all(directory),
        CLIAgent::Codex => read_codex_sessions_all(directory),
        _ => vec![],
    }
}

/// Cheap stat used as a cache version. For Claude this is the project dir's
/// mtime; for Codex the SQLite file's mtime. Returns `None` if the source
/// is missing, which callers should treat as "always reload".
pub fn source_version(agent: CLIAgent, directory: &Path) -> Option<std::time::SystemTime> {
    match agent {
        CLIAgent::Claude => {
            let project_dir = claude_projects_dir()?.join(claude_project_slug(directory));
            std::fs::metadata(&project_dir).ok()?.modified().ok()
        }
        CLIAgent::Codex => std::fs::metadata(codex_db_path()?).ok()?.modified().ok(),
        _ => None,
    }
}

pub fn read_session_starts(
    agent: CLIAgent,
    directory: &Path,
    modified_since: Option<i64>,
) -> Vec<AgentSessionFile> {
    match agent {
        CLIAgent::Claude => claude_projects_dir()
            .map(|projects_dir| {
                read_claude_session_starts(&projects_dir, directory, modified_since)
            })
            .unwrap_or_default(),
        CLIAgent::Codex => codex_db_path()
            .map(|db_path| read_codex_session_starts(&db_path, directory, modified_since))
            .unwrap_or_default(),
        _ => vec![],
    }
}

fn read_claude_session_starts(
    projects_dir: &Path,
    directory: &Path,
    modified_since: Option<i64>,
) -> Vec<AgentSessionFile> {
    let Ok(entries) = std::fs::read_dir(projects_dir.join(claude_project_slug(directory))) else {
        return vec![];
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                return None;
            }
            let session_id = path.file_stem()?.to_str()?.to_owned();
            if !is_valid_session_id(&session_id) {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            let modified_at = unix_seconds(metadata.modified().ok()?);
            if metadata.len() < 100 || modified_since.is_some_and(|since| modified_at < since) {
                return None;
            }
            Some(AgentSessionFile {
                session_id,
                created_at: claude_first_user_message_at(&path)?,
                modified_at,
            })
        })
        .collect()
}

fn claude_first_user_message_at(path: &Path) -> Option<i64> {
    use std::io::BufRead;

    let reader = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    reader
        .lines()
        .map_while(Result::ok)
        .filter(|line| line.contains("\"user\""))
        .find_map(|line| {
            let value = serde_json::from_str::<serde_json::Value>(&line).ok()?;
            if value.get("type").and_then(|t| t.as_str()) != Some("user") {
                return None;
            }
            let timestamp = value.get("timestamp")?.as_str()?;
            chrono::DateTime::parse_from_rfc3339(timestamp)
                .ok()
                .map(|dt| dt.timestamp())
        })
}

fn unix_seconds(time: std::time::SystemTime) -> i64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(diesel::QueryableByName, Debug)]
struct CodexThreadStart {
    #[diesel(sql_type = diesel::sql_types::Text)]
    id: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    created_at: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    updated_at: i64,
}

fn read_codex_session_starts(
    db_path: &Path,
    directory: &Path,
    modified_since: Option<i64>,
) -> Vec<AgentSessionFile> {
    use diesel::prelude::*;
    use diesel::sqlite::SqliteConnection;

    let Some(db_str) = db_path.to_str().map(|path| format!("file:{path}?mode=ro")) else {
        return vec![];
    };
    let Ok(mut conn) = SqliteConnection::establish(&db_str) else {
        return vec![];
    };
    diesel::sql_query(
        "SELECT id, created_at, updated_at FROM threads \
         WHERE cwd = ? AND updated_at >= ? AND first_user_message <> ''",
    )
    .bind::<diesel::sql_types::Text, _>(directory.to_string_lossy().into_owned())
    .bind::<diesel::sql_types::BigInt, _>(modified_since.unwrap_or(i64::MIN))
    .load::<CodexThreadStart>(&mut conn)
    .unwrap_or_default()
    .into_iter()
    .map(|row| AgentSessionFile {
        session_id: row.id,
        created_at: row.created_at,
        modified_at: row.updated_at,
    })
    .collect()
}

fn claude_projects_dir() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("CLAUDE_HOME") {
        return Some(PathBuf::from(home).join("projects"));
    }
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

fn claude_project_slug(directory: &Path) -> String {
    directory
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn read_claude_sessions_all(directory: &Path) -> Vec<AgentSessionEntry> {
    let Some(projects_dir) = claude_projects_dir() else {
        return vec![];
    };
    let project_dir = projects_dir.join(claude_project_slug(directory));
    let Ok(entries) = std::fs::read_dir(&project_dir) else {
        return vec![];
    };

    let mut sessions: Vec<AgentSessionEntry> = entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .filter(|e| e.metadata().map(|m| m.len() >= 100).unwrap_or(false))
        .filter_map(|e| parse_claude_session(&e.path(), directory))
        .collect();

    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions
}

fn parse_claude_session(path: &Path, directory: &Path) -> Option<AgentSessionEntry> {
    const HEAD_LINES: usize = 200;
    let session_id = path.file_stem()?.to_string_lossy().into_owned();
    let content = std::fs::read_to_string(path).ok()?;

    let mut custom_title: Option<String> = None;
    let mut title: Option<String> = None;
    let mut updated_at: Option<i64> = None;

    // Head scan: ai-title / first user message / first timestamp live near
    // the start of the file. Stop once we have both title and timestamp.
    for line in content.lines().take(HEAD_LINES) {
        if updated_at.is_some() && title.is_some() {
            break;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("ai-title") if title.is_none() => {
                title = v.get("aiTitle").and_then(|t| t.as_str()).map(str::to_owned);
            }
            Some("user") => {
                if updated_at.is_none() {
                    if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                        updated_at = chrono::DateTime::parse_from_rfc3339(ts)
                            .ok()
                            .map(|dt| dt.timestamp());
                    }
                }
                if title.is_none() {
                    if let Some(text) = extract_user_text(&v) {
                        if let Some(cleaned) = clean_user_title(&text) {
                            title = Some(truncate(&cleaned, 80));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Custom-title (from `/rename`) can appear anywhere — scan ALL lines but
    // only JSON-parse those whose raw text contains the marker. Substring
    // search is ~100× cheaper than serde_json for the discarded majority.
    for line in content.lines() {
        if !line.contains("\"custom-title\"") {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if v.get("type").and_then(|t| t.as_str()) == Some("custom-title") {
                if let Some(t) = v.get("customTitle").and_then(|t| t.as_str()) {
                    custom_title = Some(t.to_owned());
                }
            }
        }
    }

    let title = custom_title.or(title);

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

    Some(AgentSessionEntry {
        session_id,
        title,
        updated_at,
        created_at: updated_at,
        cwd: Some(directory.to_path_buf()),
        transcript_path: Some(path.to_path_buf()),
        source: CLIAgent::Claude,
        launch_argv: None,
    })
}

/// Returns a display-friendly title from a raw user-message body, or None
/// if the message is purely a Claude Code system block (slash-command caveat,
/// command-name marker, etc.) that should be skipped.
fn clean_user_title(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Skip slash-command caveat blocks injected by Claude Code.
    if trimmed.starts_with("<local-command-") || trimmed.starts_with("Caveat:") {
        return None;
    }
    // Skip pure command markers like `<command-name>/plan</command-name>...`.
    if trimmed.starts_with("<command-name>")
        || trimmed.starts_with("<command-message>")
        || trimmed.starts_with("<command-args>")
    {
        return None;
    }
    // Strip residual XML-ish tags from a real user message that just happens
    // to mention them, leaving the rest of the text.
    let stripped = strip_xml_tags(trimmed);
    let stripped = stripped.trim();
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_owned())
    }
}

fn strip_xml_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
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
    codex_home_dir().map(|h| h.join("state_5.sqlite"))
}

fn codex_home_dir() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        return Some(PathBuf::from(home));
    }
    dirs::home_dir().map(|h| h.join(".codex"))
}

fn codex_sessions_root() -> Option<PathBuf> {
    codex_home_dir().map(|h| h.join("sessions"))
}

fn find_codex_transcript_path(sessions_root: &Path, session_id: &str) -> Option<PathBuf> {
    if !sessions_root.exists() {
        return None;
    }
    let suffix = format!("-{session_id}.jsonl");
    for year_dir in read_subdirs(sessions_root) {
        for month_dir in read_subdirs(&year_dir) {
            for day_dir in read_subdirs(&month_dir) {
                let entries = std::fs::read_dir(&day_dir).ok()?;
                for entry in entries.flatten() {
                    let path = entry.path();
                    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    if name.starts_with("rollout-") && name.ends_with(&suffix) {
                        return Some(path);
                    }
                }
            }
        }
    }
    None
}

fn read_subdirs(parent: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            entry.file_type().ok()?.is_dir().then(|| entry.path())
        })
}

#[derive(diesel::QueryableByName, Debug)]
struct CodexThread {
    #[diesel(sql_type = diesel::sql_types::Text)]
    id: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    first_user_message: Option<String>,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    created_at: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    updated_at: i64,
}

fn read_codex_sessions_all(directory: &Path) -> Vec<AgentSessionEntry> {
    use diesel::prelude::*;
    use diesel::sqlite::SqliteConnection;

    let Some(db_path) = codex_db_path() else {
        return vec![];
    };
    let db_str = match db_path.to_str() {
        Some(s) => format!("file:{}?mode=ro", s),
        None => return vec![],
    };
    let Ok(mut conn) = SqliteConnection::establish(&db_str) else {
        return vec![];
    };

    let cwd = directory.to_string_lossy().into_owned();
    let sessions_root = codex_sessions_root();

    let Ok(rows) = diesel::sql_query(
        "SELECT id, first_user_message, created_at, updated_at FROM threads WHERE cwd = ? ORDER BY updated_at DESC",
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
            let transcript_path = sessions_root
                .as_deref()
                .and_then(|root| find_codex_transcript_path(root, &row.id));
            Some(AgentSessionEntry {
                session_id: row.id,
                title,
                updated_at: row.updated_at,
                created_at: row.created_at,
                cwd: Some(directory.to_path_buf()),
                transcript_path,
                source: CLIAgent::Codex,
                launch_argv: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn warp_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("app crate should live inside the repo")
            .to_path_buf()
    }

    #[test]
    #[ignore = "reads real ~/.claude/projects — run manually"]
    fn claude_sessions_found_for_warp() {
        let sessions = read_sessions(CLIAgent::Claude, &warp_dir(), "", 10);
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
        let all = read_sessions(CLIAgent::Claude, &warp_dir(), "", 10);
        let filtered = read_sessions(CLIAgent::Claude, &warp_dir(), "codex", 10);
        assert!(filtered.len() <= all.len());
        for s in &filtered {
            assert!(s.title.to_lowercase().contains("codex"));
        }
    }

    #[test]
    #[ignore = "reads real ~/.codex/state_5.sqlite — run manually"]
    fn codex_sessions_found_for_warp() {
        let sessions = read_sessions(CLIAgent::Codex, &warp_dir(), "", 10);
        assert!(!sessions.is_empty(), "expected Codex sessions for warp dir");
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
    fn claude_slug_replaces_dots() {
        let path = PathBuf::from("/Users/alice/projects/current.project/mr-assistant");
        assert_eq!(
            claude_project_slug(&path),
            "-Users-alice-projects-current-project-mr-assistant"
        );
    }

    #[test]
    fn clean_user_title_filters_caveat_and_command_blocks() {
        assert_eq!(clean_user_title("<local-command-caveat>Caveat: ..."), None);
        assert_eq!(clean_user_title("Caveat: ignore this"), None);
        assert_eq!(clean_user_title("<command-name>/plan</command-name>"), None);
        assert_eq!(clean_user_title("   "), None);
        assert_eq!(
            clean_user_title("mcp для варп видишь?").as_deref(),
            Some("mcp для варп видишь?")
        );
        assert_eq!(
            clean_user_title("hello <foo>bar</foo> baz").as_deref(),
            Some("hello bar baz")
        );
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
        let result = read_sessions(CLIAgent::Gemini, &dir, "", 10);
        assert!(result.is_empty());
    }

    #[test]
    fn claude_entry_includes_board_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript_path = tmp
            .path()
            .join("019e159b-717d-7663-9a93-95fd9c0790b1.jsonl");
        let cwd = PathBuf::from("/Users/alice/projects/warp");
        std::fs::write(
            &transcript_path,
            r#"{"type":"user","timestamp":"2026-07-07T12:34:56Z","message":{"content":[{"type":"text","text":"resume this board session"}]}}"#,
        )
        .unwrap();

        let entry = parse_claude_session(&transcript_path, &cwd).unwrap();

        assert_eq!(entry.cwd.as_deref(), Some(cwd.as_path()));
        assert_eq!(
            entry.transcript_path.as_deref(),
            Some(transcript_path.as_path())
        );
        assert_eq!(entry.source, crate::terminal::CLIAgent::Claude);
        assert_eq!(entry.launch_argv, None);
    }

    #[test]
    fn codex_transcript_path_is_discovered_from_sessions_root() {
        let tmp = tempfile::tempdir().unwrap();
        let session_id = "019e159b-717d-7663-9a93-95fd9c0790b1";
        let transcript_path = tmp
            .path()
            .join("sessions")
            .join("2026")
            .join("07")
            .join("07")
            .join(format!("rollout-2026-07-07T12-34-56-{session_id}.jsonl"));
        std::fs::create_dir_all(transcript_path.parent().unwrap()).unwrap();
        std::fs::write(&transcript_path, "{}\n").unwrap();

        assert_eq!(
            find_codex_transcript_path(&tmp.path().join("sessions"), session_id).as_deref(),
            Some(transcript_path.as_path())
        );
    }
}

#[cfg(test)]
#[path = "agent_session_reader_tests.rs"]
mod session_starts_tests;
