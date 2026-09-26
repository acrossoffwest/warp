# AI session restore after quit/crash — design

Fork feature: session memory (`app/src/session_memory/`, records in `session_memory_records`,
runs in `session_memory_app_runs`). This spec fixes how Claude Code / Codex sessions come back
after Warp closes.

## Goal

After Warp closes for any reason (Cmd-Q, force quit, crash, macOS reboot or shutdown), the next
launch relaunches every Claude Code / Codex session that was open at that moment, each in its own
pane, tab and window, resuming the same conversation. Nothing from earlier runs comes back.

## Evidence (user's database, 2026-09-26)

- Agent records of the running app had no `native_session_id` although the id was in the command
  (`claude --resume <id>`).
- `app_window_fingerprint` is empty in all 227 records.
- `live` agent records from July runs are still `live`.
- The code maps an agent that finished its turn to `success` and one asking permission to
  `blocked`; startup only restores `live`→`interrupted` records, so idle agents are skipped.

## Decisions (agreed with the user)

1. Restore after every close, including normal Cmd-Q.
2. Relaunch immediately. The setting "Automatically restore interrupted sessions" becomes
   effective and defaults to **on**; when off, the resume command is inserted into the pane's
   input without running.
3. Session ids are matched exactly; no "newest session in this folder" guessing. When no exact
   id is known, relaunch with `claude --continue` / `codex resume --last` in the same folder.
4. Approach: anchor restore to Warp's own window/tab/pane snapshot (pane uuid). Windows and tab
   positions come from the snapshot Warp already restores.

## Behavior

### What counts as an open agent session

A pane whose **currently running** command is `claude` / `codex` (any args) and whose agent
process has not exited. Turn state (working, waiting for input, asking permission, errored) does
not matter. A pane whose agent exited (`/exit`, Ctrl-C, crash of the agent) is not an open
session even if `claude` is in its history.

### Recording

- One record per terminal pane (id `warp_terminal:<pane uuid>`), stamped with the current
  `app_run_id` on every write.
- Agent lifecycle is stored separately from turn state: `live` while the agent process runs,
  `ended` (with `completed_at`) once it exits. `ended` is written to the database when the
  running agent block finishes and is never cleared by a later snapshot of the same pane unless a
  new agent command starts in that pane.
- The agent source/kind is derived from the running command, not from `last_command`.
- Session id resolution order: (1) Warp CLI-agent plugin session id; (2) id parsed from the
  running command (`--resume <id>`, `--resume=<id>`, `-r <id>`, `codex resume <id>`,
  `codex resume --last` → none); (3) the agent's own session file created in the pane's cwd after
  the command started and not already claimed by another pane. Otherwise empty.
- Launch arguments and permission mode (dangerous flags) are kept from the running command.
- Only the GUI app (`LaunchMode::App`) creates rows in `session_memory_app_runs`.

### Startup

1. The previous run is the most recent App run row before the current one (clean or not).
2. Candidates: records whose `app_run_id` is the previous run, whose agent lifecycle is `live`
   (i.e. was open at close), not closed intentionally.
3. Restore runs once, after every window from the snapshot has been created (not per window).
4. For each candidate whose pane uuid exists among restored panes (any window): run the resume
   command in that pane once its shell bootstraps.
5. Candidates without a restored pane: if Warp's layout restore (`restore_session`) is off, open
   them as new tabs in the first window; if it is on, the pane was intentionally gone — skip.
6. Resume command: `claude --resume <id>` / `codex resume <id>` with the preserved permission
   flag; without id `claude --continue` / `codex resume --last`. Run in the record's cwd (the
   restored pane already starts there; new tabs get `initial_directory`).
7. Each candidate is used at most once; after the pass the previous run's candidates are marked
   offered so a second window or a later event cannot re-run them.
8. Setting off → insert the command into the pane input without executing.
9. Removed: the 30-minute "recent agent" rule, per-window restore, the 48-hour cwd enrichment and
   the dedupe that deletes records sharing a guessed id.

### Plain terminals

Unchanged: Warp's own snapshot restore brings back shells and their cwd; session memory does not
re-run their commands automatically.

## Error handling

- Missing/corrupt record fields → the record is skipped with a log line; startup never fails.
- Resume command failing (e.g. session deleted) is visible in the pane like any command.
- Duplicate records for the same native session id in the same run: keep the one with the newest
  `last_seen_at`; never delete records of other panes.

## Testing

- Unit tests in `app/src/session_memory/*_tests.rs` for: lifecycle mapping (turn states stay
  `live`), `ended` persistence across a later snapshot upsert, id parsing from commands, session
  file matching (claimed files excluded), candidate selection (previous run only, older runs
  excluded, closed excluded, ended excluded), resume command building (id / no id, flags).
- Persistence tests (`app/src/persistence/sqlite_tests.rs`) for run-row creation and upsert that
  preserves `ended`.
- Restore targeting test: records mapped to panes across two windows go to their own panes.
- Manual on the `fc` dev profile: 2 windows with Claude/Codex panes in waiting, working and
  exited states; `kill -9`, relaunch → only open sessions come back, each in its own pane/window;
  repeat with Cmd-Q; repeat relaunch twice → nothing from the run before the last.
