# Fork control API — design

Fork-only local API that lets tools on the same machine inspect and drive Warp
tabs and panes. The consumer is a separate Node board of AI sessions (Claude
Code / Codex); it is not part of this repo.

## Goals

The board must be able to:

1. Focus the tab of an already running session instead of starting a duplicate.
2. Open a session that is not running in a new tab (`cd <dir> && claude -r <id>`).
3. Deliver a reply into a live session as if typed by the user.
4. Re-open a set of pinned sessions in tabs after a reboot.

## Naming

Everything fork-owned carries `fork` so it is never confused with upstream's
own `warpctrl` / `local_control`:

| Thing | Name |
|---|---|
| Rust crate (protocol, transport, CLI) | `crates/warp_fork_control` |
| CLI binary | `warp-fork-ctl` |
| App module | `app/src/fork_control/` |
| Hook marker in upstream files | `// fork_control:` |
| Settings group / key | `ForkControlSettings`, `fork.control_api.enabled` |
| Env: hard off | `WARP_FORK_CONTROL=0` |
| Env: socket path override | `WARP_FORK_CONTROL_SOCKET` |
| Contract doc | `docs/fork-control-api.md` |

## Transport

- Unix domain socket, newline-delimited JSON: one request line, one response line.
- macOS: `~/Library/Application Support/dev.warp.Warp[-<profile>]/fork-control/control.sock`.
  Linux: `$XDG_RUNTIME_DIR/dev.warp.Warp[-<profile>]-fork-control/control.sock`,
  falling back to the macOS-style layout under `dirs::data_local_dir()`.
- The socket directory is created with mode 0700; the socket is chmod 0600.
  Existing directories are never chmod-ed.
- The peer UID of each connection must equal the app's effective UID.
- No TCP, no network, no telemetry.
- A stale socket file is removed; a socket that still accepts connections means
  another instance is serving and startup is refused.

## Threading

Accept loop and connections run on plain `std::thread`s. Every request is sent
to the UI thread through an `async_channel` drained by `spawn_stream_local` on a
singleton model (`ForkControlHost`); the connection thread waits up to 5 s for
the answer (`timeout` otherwise). Process-table snapshots (`sysinfo`) are taken
on the connection thread, not the UI thread. Handler panics become `internal`.

## Enablement

On by default. `fork.control_api.enabled = false` in settings stops the server
live (no restart); `WARP_FORK_CONTROL=0|false|off` keeps it off regardless.

## Protocol

Request: `{"id": <any, optional>, "method": "<name>", "params": {...}}`.
Response: `{"id": ..., "api_version": 1, "ok": true, "result": ...}` or
`{"id": ..., "api_version": 1, "ok": false, "error": {"code": "...", "message": "..."}}`.

Error codes: `bad_request`, `unknown_method`, `not_found`, `not_in_tui`,
`pane_busy`, `timeout`, `unavailable`, `internal`.

IDs: `window_id` = `WindowId`, `tab_id` = `EntityId` of the tab's `PaneGroup`,
`pane_id` = `EntityId` of the pane's `TerminalView`. All are process-unique and
stable for the object's lifetime; they are not stable across app restarts.
Only terminal panes are listed.

Methods:

- `ping` → `{api_version, app_version, channel, pid}`.
- `list` → `{panes: [PaneInfo]}`; `PaneInfo` = `window_id, tab_id, tab_index,
  pane_id, title, custom_title, cwd, shell_pid, foreground_pgid,
  foreground_command, running_command, is_alt_screen, is_focused`.
- `focus {pane_id}` → activates tab + pane and brings the window/app to front.
- `open_tab {cwd, command?, title?, window: current|new, focus: bool=true}` →
  `{window_id, tab_id, pane_id, shell_pid|null}`. The command goes through the
  Warp input editor (`execute_command_or_set_pending`) so it lands in history and
  blocks; it waits for shell bootstrap.
- `set_title {tab_id | pane_id, title | null}` → custom tab title; null/empty resets.
- `send_input {pane_id, text, submit=false, mode: paste|keys, allow_shell=false}` →
  `{delivered_to: pty|input_editor}`.
  - Allowed into the PTY when a command is running in the pane (a running
    block, whether or not it uses the alternate screen — Claude Code renders in
    the main screen) or the alternate screen is active.
  - At a shell prompt: `not_in_tui` unless `allow_shell`, then the text goes
    into Warp's input editor (`submit` executes it through the editor).
  - A running Warp agent block, or a block under agent control: `pane_busy`.
  - `paste`: `\n`/`\r\n` → `\r`, wrapped in `ESC[200~ … ESC[201~` when the
    program enabled bracketed paste; `submit` sends `\r` 300 ms later (same
    delay upstream uses for CLI agents). `keys`: no wrapping, `submit` sends `\r`
    immediately.
- `find_by_pid {pid}` → the `PaneInfo` whose `shell_pid` is `pid` or an ancestor
  of it (walks parents, max depth 64), else `not_found`.

Out of scope: event subscriptions, reading screen/scrollback, closing tabs,
controlling other apps. The existing fork `remote_control` module stays untouched.

## Hook points in upstream-owned files

- `Cargo.toml` (workspace dependency), `app/Cargo.toml` (dependency).
- `app/src/lib.rs`: module declaration and host start next to `remote_control`.
- `app/src/settings/init.rs`: settings group registration.
- `crates/integration/src/test.rs`, `crates/integration/src/bin/integration.rs`:
  integration test registration.

All other code lives in fork-owned files and only calls existing public /
`pub(crate)` APIs.

## Risks

- `PaneGroup::set_title` refocuses the focused pane of that group; renaming a
  background tab must restore focus to the active tab.
- `open_tab` returns before the shell has started, so `shell_pid` may be null.
- Bracketed paste is only used when the program enabled it; otherwise
  multi-line text submits line by line.
- macOS `sun_path` limit is 104 bytes; long home paths plus a data profile could
  exceed it — startup fails with a clear log line.
