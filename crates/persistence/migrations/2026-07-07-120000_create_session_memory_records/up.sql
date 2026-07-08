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
    permission_mode TEXT NOT NULL DEFAULT 'unknown',
    last_seen_at INTEGER NOT NULL,
    started_at INTEGER,
    completed_at INTEGER,
    closed_intentionally_at INTEGER,
    restore_payload TEXT
);

CREATE INDEX session_memory_records_status_idx
    ON session_memory_records(status);

CREATE INDEX session_memory_records_source_status_idx
    ON session_memory_records(source, status);

CREATE INDEX session_memory_records_last_seen_at_idx
    ON session_memory_records(last_seen_at);
