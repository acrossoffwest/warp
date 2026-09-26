use std::path::{Path, PathBuf};

use super::{claude_project_slug, read_claude_session_starts, read_codex_session_starts};
use crate::session_memory::restore::AgentSessionFile;

#[test]
fn claude_slug_replaces_every_non_alphanumeric_character() {
    assert_eq!(
        claude_project_slug(Path::new("/Users/alice/current.project/_infra")),
        "-Users-alice-current-project--infra"
    );
    assert_eq!(
        claude_project_slug(Path::new("/Users/alice/My Project/v1.2_x")),
        "-Users-alice-My-Project-v1-2-x"
    );
    assert_eq!(
        claude_project_slug(Path::new("/Users/alice/проект")),
        "-Users-alice-------"
    );
}

const SESSION_ID: &str = "0a0a0a0a-0000-4000-8000-00000000000a";

fn claude_session_dir(projects_dir: &Path, cwd: &Path) -> PathBuf {
    let dir = projects_dir.join(claude_project_slug(cwd));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn claude_transcript(first_user_timestamp: Option<&str>) -> String {
    let mut lines = vec![
        r#"{"type":"permission-mode","permissionMode":"default","sessionId":"x"}"#.to_owned(),
        r#"{"type":"attachment","userType":"external","timestamp":"2026-07-07T12:00:00Z"}"#
            .to_owned(),
    ];
    if let Some(timestamp) = first_user_timestamp {
        lines.push(format!(
            r#"{{"type":"user","timestamp":"{timestamp}","message":{{"content":"hi"}}}}"#
        ));
    }
    lines.push("not json at all".to_owned());
    lines.join("\n")
}

#[test]
fn claude_session_starts_use_first_user_message_and_mtime() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = Path::new("/work/current.project/_infra");
    let dir = claude_session_dir(tmp.path(), cwd);
    std::fs::write(
        dir.join(format!("{SESSION_ID}.jsonl")),
        claude_transcript(Some("2026-07-07T12:34:56Z")),
    )
    .unwrap();
    std::fs::write(
        dir.join("0b0b0b0b-0000-4000-8000-00000000000b.jsonl"),
        claude_transcript(None),
    )
    .unwrap();
    std::fs::write(
        dir.join("agent-1234.jsonl"),
        claude_transcript(Some("2026-07-07T12:34:56Z")),
    )
    .unwrap();

    let starts = read_claude_session_starts(tmp.path(), cwd, None);

    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].session_id, SESSION_ID);
    assert_eq!(
        starts[0].created_at,
        chrono::DateTime::parse_from_rfc3339("2026-07-07T12:34:56Z")
            .unwrap()
            .timestamp()
    );
    assert!(starts[0].modified_at >= starts[0].created_at);
}

#[test]
fn claude_session_starts_skip_files_not_modified_since_floor() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = Path::new("/work/project");
    let dir = claude_session_dir(tmp.path(), cwd);
    std::fs::write(
        dir.join(format!("{SESSION_ID}.jsonl")),
        claude_transcript(Some("2026-07-07T12:34:56Z")),
    )
    .unwrap();
    let future = chrono::Utc::now().timestamp() + 3600;

    assert!(read_claude_session_starts(tmp.path(), cwd, Some(future)).is_empty());
}

#[test]
fn codex_session_starts_come_from_threads_rows() {
    use diesel::prelude::*;

    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("state_5.sqlite");
    let mut conn = diesel::sqlite::SqliteConnection::establish(db_path.to_str().unwrap()).unwrap();
    diesel::sql_query(
        "CREATE TABLE threads (id TEXT PRIMARY KEY, created_at INTEGER NOT NULL, \
         updated_at INTEGER NOT NULL, cwd TEXT NOT NULL, \
         first_user_message TEXT NOT NULL DEFAULT '')",
    )
    .execute(&mut conn)
    .unwrap();
    diesel::sql_query(
        "INSERT INTO threads VALUES \
         ('0a0a0a0a-0000-4000-8000-00000000000a', 100, 500, '/work/project', 'hi'), \
         ('0b0b0b0b-0000-4000-8000-00000000000b', 100, 500, '/work/other', 'hi'), \
         ('0c0c0c0c-0000-4000-8000-00000000000c', 10, 20, '/work/project', 'hi'), \
         ('0d0d0d0d-0000-4000-8000-00000000000d', 100, 500, '/work/project', '')",
    )
    .execute(&mut conn)
    .unwrap();

    let starts = read_codex_session_starts(&db_path, Path::new("/work/project"), Some(50));

    assert_eq!(
        starts,
        vec![AgentSessionFile {
            session_id: SESSION_ID.to_owned(),
            created_at: 100,
            modified_at: 500,
        }]
    );
}
