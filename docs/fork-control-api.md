# Fork control API

Contract for `crates/warp_fork_control` (protocol, transport, CLI) and its app
host, `app/src/fork_control/`. Read this document to build a client — it does
not require reading the Rust source.

## 1. What it is

A fork-only, local-only API that lets other processes on the same machine
inspect and drive Warp's tabs and panes: list panes, focus one, open a new
tab, set a tab's title, type/paste text into a pane, and find the pane that
owns a given PID.

It is **not** upstream Warp's `warpctrl` / `local_control`. It is a separate,
fork-owned addition, reachable only from processes running as the same OS
user on the same machine — no network, no telemetry, no remote access. It
also does not replace or interact with this fork's pre-existing
`remote_control` module.

The intended consumer is an external tool such as a Node.js "board of AI
sessions" that starts, resumes and messages `claude` / `codex` CLI sessions
running inside Warp panes.

## 2. Socket

Transport is a Unix domain socket, newline-delimited JSON: one JSON object
per line in each direction. There is no TCP/network listener.

**Default path** (no `WARP_FORK_CONTROL_SOCKET` set):

- macOS: `~/Library/Application Support/dev.warp.Warp[-<profile>]/fork-control/control.sock`
- Linux, `$XDG_RUNTIME_DIR` set: `$XDG_RUNTIME_DIR/dev.warp.Warp[-<profile>]-fork-control/control.sock`
- Linux, `$XDG_RUNTIME_DIR` unset: falls back to the macOS-style layout under
  `dirs::data_local_dir()` (typically `~/.local/share/dev.warp.Warp[-<profile>]/fork-control/control.sock`)

`[-<profile>]` is `-<value of WARP_DATA_PROFILE>` when a data profile is
active, otherwise omitted (see "Data profiles" below). The directory name is
always `dev.warp.Warp[-<profile>]`, independent of release channel (stable,
preview, dev, oss, …).

**Override:** the environment variable `WARP_FORK_CONTROL_SOCKET` — read by
both the app and `warp-fork-ctl` — replaces the whole path above.
`warp-fork-ctl` resolution order is: `--socket` flag, then
`$WARP_FORK_CONTROL_SOCKET`, then the profile default. `--profile` /
`WARP_DATA_PROFILE` is therefore ignored whenever `WARP_FORK_CONTROL_SOCKET`
is set.

**Data profiles:** `WARP_DATA_PROFILE` only takes effect in debug/dev builds
of the **app** (`cfg!(debug_assertions)`); an empty value is treated as
unset. **A release-build app always uses the default (no-profile) path
regardless of this variable.** `warp-fork-ctl` itself has no such guard —
its `--profile` flag / inherited `WARP_DATA_PROFILE` env var is honored
unconditionally, in both debug and release builds of the CLI. The trap this
creates: if `WARP_DATA_PROFILE` is set in your shell (e.g. left over from
dev work) and you run a release-build `warp-fork-ctl` against a
release-build Warp, the CLI computes the profiled socket path while the app
is listening on the default one — `ping` fails to connect. Unset the
variable (or don't pass `--profile`), or pass `--socket` explicitly, when
targeting a release build.

**Permissions and ownership:**
- The socket's parent directory is created with mode `0700` if it doesn't
  already exist (an existing directory's mode is never changed).
- The socket file itself is `chmod 0600` right after `bind`.
- Every accepted connection is checked for the peer's UID (`getpeereid` on
  macOS/BSD, `SO_PEERCRED` on Linux); a connection from a different UID is
  logged and dropped without any response. On platforms where the peer UID
  cannot be determined, the connection is rejected the same way.
- Socket paths are limited to 103 bytes on both macOS and Linux (macOS's
  `sun_path` is 104 bytes including the NUL; Linux's is 108, but the same
  103-byte cap is enforced there too). A longer path (e.g. a long home
  directory plus a long profile name) makes startup fail with a clear log
  line — the server is simply not started, `ping` will fail to connect.

**Stale sockets and instance conflicts:**
- If a file already exists at the socket path and connecting to it succeeds,
  another instance is actively serving it — the new instance refuses to
  start (logged, no crash).
- If a file exists but nothing answers on it (a stale socket left behind by
  a hard kill/SIGKILL/SIGTERM/crash — the socket file is **not** removed by
  a non-graceful exit), the new instance removes it and binds fresh.
- If a non-socket file exists at that path, the new instance refuses to
  start rather than delete it.

## 3. Enable / disable

The API is **on by default**.

- **Settings file** — key `fork.control_api.enabled` (TOML dotted path,
  boolean, default `true`) in Warp's `settings.toml`. There is currently no
  Settings-UI toggle wired up for this key (unlike some other settings
  groups) — set it by editing the file directly:
  ```toml
  [fork.control_api]
  enabled = false
  ```
  Even so, it applies live: the running app watches this settings model and
  starts/stops the server as soon as the file changes, no restart needed.
- **Environment variable** — `WARP_FORK_CONTROL`, read by the **Warp app
  process itself** (not by any client), forces the server off regardless of
  the setting when its value is exactly (lowercase) `0`, `false`, or `off`.
  Any other value, or unset, defers to the setting above. Since it's read
  from the app's own environment at the point it evaluates the combined
  enabled/disabled state, it has no effect if set only in a client's shell
  after Warp has already launched.

When disabled, there is no socket at all; connecting fails with a normal
"no such file" / connection-refused error, not a protocol-level error.

## 4. Wire format

One connection may carry many requests. Each line you write is one JSON
request object; each line the server writes back is one JSON response
object, in the same order the requests were received on that connection —
requests on one connection are answered strictly in order (there is one
handler thread per connection, and each request round-trips through Warp's
UI thread before the next is read). Concurrency across sessions means
opening a separate connection per concurrent caller, or pipelining and
matching responses by `id`.

**Request:**

```json
{"id": 1, "method": "ping", "params": {}}
```

- `id` — optional, any JSON value (client's choice: number, string, or
  omitted). Echoed back verbatim in the response. If omitted, the response's
  `id` is `null`. Note `0` is a valid, distinct id — don't treat a falsy `id`
  as "no id" in client code.
- `method` — required string.
- `params` — optional object; omitted is equivalent to `{}`. `ping` and
  `list` ignore it entirely. Every other method's params type rejects
  unknown fields with `bad_request` — typos in a field name are caught, not
  silently ignored.

**Response, success:**

```json
{"id": 1, "api_version": 1, "ok": true, "result": {}}
```

**Response, error:**

```json
{"id": 1, "api_version": 1, "ok": false, "error": {"code": "not_found", "message": "..."}}
```

`api_version` is always `1` (this document's contract version) on every
response, success or error. A malformed line (invalid JSON, JSON that isn't
an object, a missing/non-string `method`) still gets an error response with
`code: "bad_request"` and does **not** close the connection — keep reading
after an error. Two exceptions worth knowing:
- A **blank line** (empty after trimming) is silently skipped: no response
  is written for it. Don't count "one response per line sent" if you ever
  write blank lines.
- A line that isn't valid UTF-8 makes the underlying line reader fail and
  **does** end the connection (Rust's `BufRead::lines()` errors on invalid
  UTF-8). Keep all request text UTF-8.

**Key order:** responses are serialized by `serde_json` without a
preserve-order feature, so object keys come out in ascending alphabetical
order (confirmed against live output below), not struct-declaration order.
This is not part of the contract — parse by key name, never by position.

## 5. Methods

Each subsection gives the params/result Rust types translated to a plain
table, the errors the method can produce beyond the generic ones (a bad
socket line always risks `bad_request`; a disabled/overloaded server always
risks `unavailable`; any handler bug risks `internal`), one example, and the
matching `warp-fork-ctl` subcommand (captured from
`warp-fork-ctl <subcommand> --help` in this fork's own build; flags are
exact, descriptions of positional args are blank in `clap`'s own output).

`warp-fork-ctl`'s own error convention (from `src/bin/warp_fork_ctl.rs`, not
part of the socket protocol itself): a protocol-level error prints
`<code>: <message>` to stderr and exits `1`; a transport/usage failure
(can't connect, bad arguments, etc.) prints a `warp-fork-ctl: ...` message
and exits `2`. Success pretty-prints the `result` object to stdout (`list`
without `--json` prints a table instead).

### `ping`

No params.

**Result:**

| Field | Type | Meaning |
|---|---|---|
| `api_version` | `u32` | Same value as the envelope's `api_version` (always `1`) |
| `app_version` | `string \| null` | Warp's app version, or `null` if unavailable |
| `channel` | `string` | Release channel, lowercased (`"oss"`, `"stable"`, `"preview"`, `"dev"`, `"integration"`, …) |
| `pid` | `u32` | PID of the Warp process answering |

**Errors:** none beyond the generic ones.

**Example — live full envelope, from the Task 11 integration test** (an
`integration`-channel test build):

Request: `{"id":1,"method":"ping","params":{}}`
Response: `{"api_version":1,"id":1,"ok":true,"result":{"api_version":1,"app_version":null,"channel":"integration","pid":61492}}`

CLI: `warp-fork-ctl ping` (pretty-prints just the `result` object — a live
run against an OSS build printed
`{"api_version": 1, "app_version": null, "channel": "oss", "pid": 82929}`,
`pid` matching the running app's own PID exactly).

### `list`

No params.

**Result:** `{"panes": [PaneInfo, ...]}`

**`PaneInfo` fields:**

| Field | Type | Meaning |
|---|---|---|
| `window_id` | `u64` | Process-unique window id |
| `tab_id` | `u64` | Process-unique id of the tab's pane group |
| `tab_index` | `usize` | 0-based tab position within its window |
| `pane_id` | `u64` | Process-unique id of the pane's terminal view — the id every other method keys on |
| `title` | `string` | Effective displayed tab title (custom title if set, else the derived one) |
| `custom_title` | `string \| null` | User/API-set title only, `null` if none is set |
| `cwd` | `string \| null` | Current working directory of the pane's shell, `null` for a non-local session |
| `shell_pid` | `u32 \| null` | PID of the pane's shell process; `null` if the shell hasn't started yet |
| `foreground_pgid` | `u32 \| null` | PID of the process group currently in the foreground of the pane's PTY |
| `foreground_command` | `string \| null` | OS process name (not a full command line) of `foreground_pgid`'s leader |
| `running_command` | `string \| null` | The command text of the block Warp currently considers "running" in this pane, if any |
| `is_alt_screen` | `bool` | Whether the pane's terminal is in the alternate screen (full-screen TUI apps use this) |
| `is_focused` | `bool` | Whether this pane is the one currently focused (its window is the active OS window, its tab is the active tab, and it is the focused pane within that tab) |

Only terminal panes are included, and only ones Warp itself currently
considers visible (its own split/zoom UI state can hide a pane — mid-close,
mid-move, or otherwise hidden — the same filter the GUI uses). Order is by
window, then tab index, then pane order within the tab.

**Errors:** none beyond the generic ones.

**Example — live, from the Task 11 integration test** (path replaced with a
placeholder; a fresh session normally has exactly one pane):

Request: `{"id":1,"method":"list","params":{}}`
Response:
```json
{"api_version":1,"id":1,"ok":true,"result":{"panes":[{"custom_title":null,"cwd":"/Users/<user>/tmp/test_fork_control_api","foreground_command":"zsh","foreground_pgid":61700,"is_alt_screen":false,"is_focused":true,"pane_id":1665,"running_command":null,"shell_pid":61700,"tab_id":1657,"tab_index":0,"title":"~","window_id":0}]}}
```

A logged-out or not-yet-onboarded Warp instance (no `Workspace` created yet)
answers with `{"panes": []}` rather than an error — confirmed live against a
fresh, never-logged-in profile.

CLI: `warp-fork-ctl list` (table) or `warp-fork-ctl list --json` (raw
`result`).

### `focus`

**Params:**

| Field | Type | Default | Meaning |
|---|---|---|---|
| `pane_id` | `u64` | required | Pane to focus |

Activates the pane's tab, focuses the pane within it, and brings the pane's
window to the front and the app itself to the front (equivalent to clicking
it).

**Result:** `{}` (empty object) on success.

**Errors:** `not_found` if no pane has that `pane_id`.

**Example — live error, from the Task 11 integration test** (a nonexistent
`pane_id`):

Request: `{"id":1,"method":"focus","params":{"pane_id":999999999}}`
Response: `{"api_version":1,"error":{"code":"not_found","message":"no terminal pane with pane_id 999999999"},"id":1,"ok":false}`

CLI: `warp-fork-ctl focus <PANE_ID>`.

### `open_tab`

**Params:**

| Field | Type | Default | Meaning |
|---|---|---|---|
| `cwd` | `string` | required | Absolute, existing directory. Not tilde-expanded — `~/foo` is rejected. |
| `command` | `string \| null` | `null` | Shell command line run in the new pane once its shell is ready. Whitespace-only is treated as absent. Caller is responsible for any quoting. |
| `title` | `string \| null` | `null` | Custom tab title. Whitespace-only is treated as absent (no title set). |
| `window` | `"current" \| "new"` | `"current"` | Open in the current/target window, or create a new window |
| `focus` | `bool` | `true` | Whether to activate the new tab / bring the window and app to front |

**Result:**

| Field | Type | Meaning |
|---|---|---|
| `window_id` | `u64` | Window the tab was opened in (newly created if `window: "new"`) |
| `tab_id` | `u64` | New tab's pane-group id |
| `pane_id` | `u64` | New tab's (single) pane id |
| `shell_pid` | `u32 \| null` | PID of the new pane's shell, or `null` — see Guarantees below |

The command goes through Warp's normal input editor path
(`execute_command_or_set_pending`), so it lands in the pane's shell history
like anything the user typed, and is deferred until the shell's login
bootstrap completes if it isn't ready yet — `open_tab` itself does not wait
for that; see Guarantees.

**Window selection details:**
- `window: "current"` targets: the active window, else the frontmost window,
  else the first window by id (lowest id first) — whichever exists.
  If **no** Warp window is open at all, this fails with `unavailable`
  ("no Warp window is open; use window \"new\"").
- `window: "new"` always creates a window and **always brings it to front**
  — the `focus` flag is ignored for a new window. If there is no Warp
  window open and the user is logged out (no active session), window
  creation can itself fail; that surfaces as `internal` ("new window has
  no workspace").
- With `window: "current"` and `focus: false`, the tab is created but the
  previously active tab (identified by its tab, not its position) is
  re-activated afterward — the new tab exists but isn't the one shown.
  Adding and re-activating a tab goes through Warp's normal tab switching,
  which closes an open command palette in that window and focuses the
  active tab's pane.

**Errors:**
- `bad_request` — `cwd` isn't an absolute, existing directory.
- `unavailable` — `window: "current"` with no Warp window open.
- `internal` — `window: "new"` and no workspace could be created (e.g.
  logged out with no window to fall back to).

**Example — live, from the Task 11 integration test** (`window`/`focus`
omitted, so their defaults apply; `shell_pid: null` because the response
returns before the shell finishes starting):

Request: `{"id":1,"method":"open_tab","params":{"cwd":"/Users/<user>/tmp/test_fork_control_api","command":"echo fork-control","title":"FC-TEST"}}`
Response: `{"api_version":1,"id":1,"ok":true,"result":{"pane_id":2239,"shell_pid":null,"tab_id":2231,"window_id":0}}`

CLI: `warp-fork-ctl open --cwd <CWD> [--cmd <CMD>] [--title <TITLE>] [--new-window] [--no-focus]`.

### `set_title`

**Params:**

| Field | Type | Default | Meaning |
|---|---|---|---|
| `tab_id` | `u64 \| null` | `null` | Target tab, by tab id |
| `pane_id` | `u64 \| null` | `null` | Target tab, by one of its pane ids |
| `title` | `string \| null` | `null` | New title; `null` (including omitted, which deserializes the same as `null`) clears the custom title back to the derived one |

Exactly one of `tab_id`/`pane_id` must be given — both present or both
absent is `bad_request`. `title` is trimmed; an empty or whitespace-only
title clears the custom title the same as `null` does.

**Result:** `{}` on success.

**Errors:** `bad_request` (neither or both of `tab_id`/`pane_id` given),
`not_found` (no tab/pane with that id).

Renaming never changes the active tab. The upstream title setter focuses
the renamed tab's pane as a side effect; Warp then hands focus back to the
active tab's focused pane (without switching tabs or closing anything).
Residual effect: if keyboard focus was somewhere other than the active
tab's focused pane — an open command palette, a search bar, a dialog in
that window — that element stays open but loses keyboard focus to the
active tab's pane, so the user has to click back into it.

**Example — built from the types** (no live sample exists for this method;
values are placeholders):

Request: `{"id":1,"method":"set_title","params":{"tab_id":1657,"title":"claude — refactor auth"}}`
Response: `{"api_version":1,"id":1,"ok":true,"result":{}}`

CLI: `warp-fork-ctl title <ID> [TITLE] [--pane] [--clear]` (`<ID>` is a
`tab_id` unless `--pane` is given, then it's a `pane_id`; omit `TITLE` and
pass `--clear` to reset).

### `send_input`

**Params:**

| Field | Type | Default | Meaning |
|---|---|---|---|
| `pane_id` | `u64` | required | Target pane |
| `text` | `string` | required | Text to deliver |
| `submit` | `bool` | `false` | Also send an Enter / execute the text as a command |
| `mode` | `"paste" \| "keys"` | `"paste"` | See section 8 |
| `allow_shell` | `bool` | `false` | Permit delivery when the pane is at a shell prompt (not inside a running program) |

**Result:** `{"delivered_to": "pty" | "input_editor"}`.

**Errors:** `not_in_tui` (pane is at a shell prompt and `allow_shell` is
`false`), `pane_busy` (the pane is running a Warp-agent block, or a Warp
agent controls the running command's input — no text reaches the pane),
`not_found` (`pane_id` doesn't exist). Full routing/encoding rules are in
section 8.

**Example — live error, from the Task 11 integration test** (a fresh pane at
a shell prompt, `allow_shell` omitted/`false`):

Request: `{"id":1,"method":"send_input","params":{"pane_id":1665,"text":"x"}}`
Response: `{"api_version":1,"error":{"code":"not_in_tui","message":"pane is at a shell prompt; pass allow_shell to type into the Warp input editor"},"id":1,"ok":false}`

**Example — success, built from the types** (a pane running an interactive
program, e.g. `claude`, delivered to its PTY):

Request: `{"id":2,"method":"send_input","params":{"pane_id":1665,"text":"continue with step 2","submit":true}}`
Response: `{"api_version":1,"id":2,"ok":true,"result":{"delivered_to":"pty"}}`

CLI: `warp-fork-ctl send <PANE_ID> <TEXT> [--submit] [--allow-shell] [--keys]`
(pass `TEXT` as `-` to read it from stdin).

### `find_by_pid`

**Params:**

| Field | Type | Default | Meaning |
|---|---|---|---|
| `pid` | `u32` | required | Any PID inside the pane's shell's process tree |

**Result:** a single `PaneInfo` (same shape as one element of `list`'s
`panes` array — **not** wrapped in an object).

Walks up the process tree from `pid` through its parents (up to 64 levels,
stopping at PID 0 or a self-referencing parent) looking for a PID that
matches some pane's `shell_pid`. This is how a board that only knows a
`claude`/`codex` child PID (not the shell PID) finds its owning pane.

**Errors:** `not_found` — no pane's shell is `pid` or an ancestor of it
within the walked depth.

**Example — success, built from the types** (`pid` is the `claude` child
process; the walk finds its ancestor shell at `shell_pid: 61700` and returns
that pane):

Request: `{"id":1,"method":"find_by_pid","params":{"pid":61905}}`
Response: `{"api_version":1,"id":1,"ok":true,"result":{"custom_title":null,"cwd":"/Users/<user>/projects/app","foreground_command":"claude","foreground_pgid":61905,"is_alt_screen":false,"is_focused":true,"pane_id":1665,"running_command":"claude -r <session-id>","shell_pid":61700,"tab_id":1657,"tab_index":0,"title":"~","window_id":0}}`

**Example — live error, from the Task 7 smoke test** (a fresh, empty pane
set — `find-pid` on any pid returns `not_found`):

```
$ warp-fork-ctl find-pid 1
not_found: no pane owns pid 1
```
(exit code 1 — see the CLI error convention at the top of this section).

CLI: `warp-fork-ctl find-pid <PID>`.

## 6. Error codes

| Code | When |
|---|---|
| `bad_request` | Malformed request line (invalid JSON, not an object, missing/non-string `method`, unknown field inside `params`, wrong param type); `set_title` with neither or both of `tab_id`/`pane_id`; `open_tab` with a `cwd` that isn't an absolute, existing directory |
| `unknown_method` | `method` isn't one of the seven names above |
| `not_found` | `focus`/`set_title`/`send_input` given a `pane_id`/`tab_id` that doesn't exist; `find_by_pid` found no owning pane |
| `not_in_tui` | `send_input` on a pane at a shell prompt without `allow_shell: true` |
| `pane_busy` | `send_input` on a pane where a Warp agent block is running, or where a Warp agent controls the running command's input — in both cases no text is delivered |
| `timeout` | The UI thread did not answer within 5 seconds (see Guarantees for exactly when the request can still take effect) |
| `unavailable` | The API is disabled (including a race where it was disabled after the request was accepted); the job queue is closed (Warp shutting down); `open_tab` with `window: "current"` and no Warp window open |
| `internal` | A request handler panicked; a result failed to serialize; `open_tab` with `window: "new"` could not create a workspace; the reply channel was dropped before answering |

## 7. Guarantees

- **ID stability:** `window_id`, `tab_id`, `pane_id` are unique within the
  running process and stable for as long as the underlying window/tab/pane
  exists — they are **not** stable across an app restart. Don't persist
  them across sessions; re-resolve panes via `find_by_pid` or `list` after
  a relaunch.
- **Scope:** only terminal panes are ever listed or addressable — there is
  no way to enumerate or affect any other kind of view.
- **Threading:** every request that touches app state runs on Warp's UI
  thread, one at a time, in the order the connection's handler thread reads
  it off the socket. `list`/`find_by_pid` process-table snapshots are taken
  on the connection thread beforehand (not the UI thread), so they can be
  slightly stale relative to the exact moment the UI thread processes the
  request.
- **5-second UI timeout:** every request gets a deadline 5 seconds after it
  is queued; the connection waits exactly until that deadline. When the
  UI thread picks up a job whose deadline has passed, it drops it without
  running it. A `timeout` response therefore means one of two things:
  (a) the job had not started by the deadline and will never run, or
  (b) its handler had already started before the deadline and was still
  executing when the connection stopped waiting — it then completes
  normally, but its answer is discarded. Case (b) needs the UI thread to be
  stuck inside that very handler, which is rare; case (a) is what a busy or
  blocked UI thread produces. Retrying after `timeout` is safe except in
  case (b), which the caller cannot distinguish from (a); for `open_tab`,
  check `list` for a matching pane before retrying so case (b) cannot
  produce a second tab. Timers a handler started (the delayed Enter of
  `send_input`, an `open_tab` command waiting for the shell) are not
  affected by the deadline.
- **Handler panics:** a request handler that panics is caught and answered
  with `internal`; the API and the app keep running. Handlers are written
  not to panic, but a panic that happens inside a nested update of a view
  (a pane, a tab, the workspace) can leave that particular view in an
  inconsistent state until it is closed.
- **`open_tab` returns before the shell starts:** `shell_pid` in the result
  can be `null`; the requested `command` still runs, once the new pane's
  shell finishes its login bootstrap. A caller that needs the shell PID (or
  needs to know the command actually started) should poll `list` for that
  `pane_id` until `shell_pid` is non-null / `running_command` is set,
  rather than treating `open_tab`'s response as "done".

## 8. `send_input` rules

Routing, in order:

1. If the pane's terminal is on the **alternate screen** (full-screen TUI
   apps such as `vim` or `less` use this), input always goes to the **PTY**,
   regardless of `allow_shell`.
2. Else, if a **Warp agent** block is the pane's active block, the request
   fails with `pane_busy` — this holds even if `allow_shell` is set. This
   is checked before rule 3, so a Warp agent block wins over a running
   command.
3. Else, if Warp considers a command **running** in the pane (a running
   block that hasn't completed yet — this is the path Claude Code and other
   agents rendering in the main screen normally take, since they don't use
   the alt screen), input goes to the **PTY**.
4. Else the pane is **at a shell prompt**: without `allow_shell` this fails
   with `not_in_tui`; with `allow_shell: true` it is delivered to Warp's own
   **input editor** instead of the PTY.

On the PTY route, if a Warp agent controls the running command's input
(Warp's "agent in control" state for a long-running command), the call
fails with `pane_busy` and nothing is written. This is checked right
before the first write; no partial text is ever delivered.

**PTY delivery (`delivered_to: "pty"`), `submit: true` to a recognized CLI
agent:** when `submit` is `true`, `text` is non-empty and Warp has
recognized a CLI agent session in the pane (Claude Code, Codex, Gemini,
Copilot, OpenCode, … — the same detection that drives Warp's agent
footer/rich input), the text is handed to Warp's own per-agent submit
pipeline, the one its rich-input composer uses. `mode` and newline
normalization do not apply on this path; the agent's strategy decides:
- Claude Code, Gemini, OpenCode, Auggie, Grok, Cursor CLI: unwrapped text,
  then `\r` about 50 ms later (they ignore a `\r` that arrives together
  with the text).
- Codex, Hermes, OhMyPi: bracketed paste, then `\r` immediately.
- Copilot: bracketed paste, then `\r` about 300 ms later.
- Other recognized agents: text and `\r` in one write.

Delivering through that pipeline may also close the pane's CLI-agent rich
input composer if the user has it open (Warp's own "dismiss after submit"
setting decides, as for a normal submit).

**PTY delivery, everything else** (no recognized CLI agent, `submit:
false`, or empty `text`):
- `mode: "paste"` (default): `\r\n` and `\n` in `text` are normalized to
  `\r`. If the target program has itself enabled **bracketed paste**, the
  text is wrapped in `ESC[200~ … ESC[201~`; otherwise it is written as
  plain normalized bytes (a multi-line paste to a program that never asked
  for bracketed paste therefore submits/executes line by line as the
  terminal sees each `\r`).
- `mode: "keys"`: never wrapped in bracketed-paste markers, regardless of
  what the program enabled; `\r\n`/`\n` are still normalized to `\r`.
- `submit: true` with non-empty text: the terminating `\r` is **always** a
  separate write, sent **300 ms** after the text if the text was wrapped in
  bracketed paste, otherwise **50 ms** after it. Many TUIs ignore an Enter
  that arrives in the same read as the text, or right after a paste end
  marker.
- `submit: true` with empty `text`: just `\r`, written immediately.

The delayed Enter (either path) is fire-and-forget: the call has already
returned `ok: true` when it fires. If the pane is closed or a Warp agent
takes control of it in that window, the Enter is silently dropped and the
caller cannot observe it.

**Input-editor delivery (`delivered_to: "input_editor"`, i.e. `allow_shell`
routed a shell-prompt pane):** `mode` is ignored — there is no PTY wrapping
on this path.
- `submit: false`: `text` is inserted at the input editor's current cursor
  position, **appending** to whatever text is already in the editor (it
  does not replace or clear existing content).
- `submit: true`: `text` is likewise inserted at the cursor (appending to
  any existing editor content), and then the **combined** editor text is
  submitted for execution as one command — immediately if the pane's shell
  has finished its login bootstrap, otherwise deferred until it has.

`send_input` never returns a body describing what, if anything, was already
in the editor — a caller that cares should clear/read it first via its own
means, or avoid relying on an empty editor.

## 9. Recipes for the session board

**(a) Focus an already-running session**, given the child PID recorded when
the session was launched:
```json
{"method": "find_by_pid", "params": {"pid": 54321}}
```
→ take `pane_id` from the result, then:
```json
{"method": "focus", "params": {"pane_id": 1665}}
```

**(b) Open a session that isn't currently running**, resuming it by id:
```json
{"method": "open_tab", "params": {"cwd": "/Users/<user>/projects/app", "command": "claude -r <session-id>", "title": "claude — app"}}
```
Poll `list` for the returned `pane_id` until `shell_pid` is set (or
`running_command` reflects the `claude` process) before assuming the agent
process has actually started — see the `open_tab` note in Guarantees.

**(c) Deliver a reply into a live session**, as if typed by the user:
```json
{"method": "find_by_pid", "params": {"pid": 54321}}
```
→ then:
```json
{"method": "send_input", "params": {"pane_id": 1665, "text": "please also update the tests", "submit": true}}
```
No `allow_shell` is needed here — the target pane is expected to already be
running the agent (alt screen or a running block), so this routes to the
PTY; for a pane where Warp recognizes Claude Code, Codex etc. it goes
through that agent's own submit strategy (section 8). Every `submit: true`
sends its Enter up to 300 ms after the call returns, so space consecutive
`send_input` calls to the same pane apart by more than 300 ms, or a pending
Enter can interleave with the next call's text.

**(d) Restore pinned sessions after a reboot.** `ping` first — the API can't
launch Warp itself, so wait for it to be up. Then `list` and match already-
running panes against the board's saved sessions (by `cwd` and/or
`running_command`) before opening anything: Warp's own window restore and
this fork's Session Memory feature can already have brought some of them
back, and looping `open_tab` unconditionally would create duplicates. Only
for sessions with no matching pane, one `open_tab` call each:
```json
{"method": "open_tab", "params": {"cwd": "/Users/<user>/projects/app", "command": "claude -r <session-id>", "window": "new"}}
```
(`window: "new"` for the first one to get a window at all if none is open;
`"current"` — the default — for subsequent tabs once a window exists.) Do
not retry an individual `open_tab` call on `timeout` without first checking
`list` for a pane that already matches it — a timed-out request can still
have run if its handler was already executing at the deadline (see
Guarantees).

## 10. Minimal Node client

Requires only Node's built-in `net` module.

```js
const net = require('net');

function connectForkControl(socketPath) {
  const socket = net.createConnection(socketPath);
  socket.setEncoding('utf8');
  let buffer = '';
  let nextId = 1;
  const pending = new Map();

  function fail(err) {
    for (const { reject } of pending.values()) reject(err);
    pending.clear();
  }
  socket.on('error', fail);
  socket.on('close', () => fail(new Error('fork-control socket closed')));
  socket.on('data', (chunk) => {
    buffer += chunk;
    let nl;
    while ((nl = buffer.indexOf('\n')) !== -1) {
      const line = buffer.slice(0, nl);
      buffer = buffer.slice(nl + 1);
      if (!line.trim()) continue;
      const res = JSON.parse(line);
      const waiter = pending.get(res.id);
      if (!waiter) continue;
      pending.delete(res.id);
      if (res.ok) waiter.resolve(res.result);
      else waiter.reject(Object.assign(new Error(res.error.message), { code: res.error.code }));
    }
  });

  return (method, params = {}) => new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    socket.write(JSON.stringify({ id, method, params }) + '\n');
  });
}

module.exports = { connectForkControl };
```

Usage: `const call = connectForkControl(socketPath); const { pid } = await call('ping');`

The socket path itself is not discovered by this client — pass
`process.env.WARP_FORK_CONTROL_SOCKET` when set, otherwise the platform
default path from section 2 (this fork does not expose a way to ask Warp
for its own socket path other than by successfully connecting to it).

## 11. Limitations

- No event subscriptions or push notifications of any kind — the board must
  poll `list` (and its own PID bookkeeping) to notice new panes, closed
  panes, or state changes.
- No screen or scrollback reading — there is no way to see what a pane has
  printed, only whether it is on the alt screen and what its recorded
  `running_command` text is.
- No tab or pane closing, splitting, or moving.
- No control over any other application.
- macOS and Linux only; `warp-fork-ctl` refuses to run on any other OS
  (Windows), and there is no Windows transport.
- Bracketed-paste wrapping on `send_input` only happens when the target
  *program* has itself enabled bracketed paste (a terminal capability the
  program opts into, e.g. most readline/editline-based programs and TUIs);
  a program that never enabled it receives unwrapped, newline-normalized
  text, and a multi-line paste submits/executes line by line rather than
  as one atomic paste. (A `submit: true` to a recognized CLI agent follows
  that agent's own strategy instead — section 8.)
- `open_tab`'s `focus` flag has no effect when `window: "new"` — a new
  window is always brought to front.
- No exactly-once delivery guarantee across a `timeout`: an expired request
  is dropped, but one whose handler was already running at the deadline
  still completes — see Guarantees.
