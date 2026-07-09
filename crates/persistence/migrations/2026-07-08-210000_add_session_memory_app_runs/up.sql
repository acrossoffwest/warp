ALTER TABLE session_memory_records ADD COLUMN app_run_id TEXT;
ALTER TABLE session_memory_records ADD COLUMN recovery_offered_run_id TEXT;

CREATE TABLE session_memory_app_runs (
    run_id TEXT PRIMARY KEY NOT NULL,
    started_at INTEGER NOT NULL,
    clean_shutdown_at INTEGER
);

CREATE INDEX session_memory_records_app_run_idx
    ON session_memory_records(app_run_id);

CREATE INDEX session_memory_records_recovery_offered_run_idx
    ON session_memory_records(recovery_offered_run_id);

CREATE INDEX session_memory_app_runs_started_at_idx
    ON session_memory_app_runs(started_at);
