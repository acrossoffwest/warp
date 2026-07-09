DROP INDEX IF EXISTS session_memory_app_runs_started_at_idx;
DROP INDEX IF EXISTS session_memory_records_recovery_offered_run_idx;
DROP INDEX IF EXISTS session_memory_records_app_run_idx;
DROP TABLE IF EXISTS session_memory_app_runs;

ALTER TABLE session_memory_records DROP COLUMN recovery_offered_run_id;
ALTER TABLE session_memory_records DROP COLUMN app_run_id;
