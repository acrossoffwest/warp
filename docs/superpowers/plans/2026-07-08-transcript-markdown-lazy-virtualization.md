# Transcript Markdown, Lazy Parsing, And Virtualized Rendering Plan

## Goal

Make Session Memory transcript panes fast and readable for large Claude Code/Codex transcripts.

The transcript pane must:

- Render user/assistant messages as Markdown.
- Render GFM-style Markdown tables correctly.
- Avoid UI freezes on large JSONL transcript files.
- Avoid constructing UI elements for every transcript message on every render.
- Keep the current safety filters: no tool calls, raw JSON events, AGENTS.md bootstrap text, system reminders, or command wrapper noise in the chat view.

## Current State

`app/src/workspace/view/session_memory_transcript.rs` currently does three expensive things synchronously:

- `fs::read_to_string(path)` loads the whole transcript into memory.
- `parse_transcript(&contents)` parses every JSONL line immediately into `Vec<TranscriptMessage>`.
- `render_messages()` builds every message card every render inside one `Flex::column`.

Markdown is not rendered yet; message content is plain `Text`.

The repo already has useful primitives:

- `crates/markdown_parser` parses Markdown into `FormattedTextLine`, including `FormattedTextLine::Table(FormattedTable)`.
- `FormattedTable` contains headers, alignments, and rows.
- `warpui_core::elements::viewported_list::{List, ListState}` provides variable-height virtualized list rendering.
- `warpui_core::elements::table::{Table, TableStateHandle}` provides table virtualization, but this is for whole table widgets, not directly for transcript message lists.

## Architecture

Split transcript rendering into three layers:

1. `TranscriptLoader`: background/incremental JSONL reader.
2. `TranscriptMessageStore`: appendable message state plus load/error/cancel status.
3. `TranscriptRenderer`: virtualized message list, with per-visible-message Markdown parsing/cache.

The transcript pane should no longer own only `TranscriptLoadResult::Loaded(Vec<TranscriptMessage>)`. It should own a state object roughly like:

```rust
struct TranscriptState {
    path: PathBuf,
    status: TranscriptLoadStatus,
    messages: Vec<TranscriptMessage>,
    total_lines_seen: usize,
    parse_errors: usize,
    reached_start: bool,
    reached_end: bool,
    markdown_cache: LruCache<TranscriptMessageKey, ParsedMarkdownMessage>,
}
```

Message identity should be stable enough for caching:

```rust
struct TranscriptMessage {
    id: TranscriptMessageId,
    role: TranscriptRole,
    content: String,
    timestamp: Option<String>,
    source_line: usize,
}
```

`id` can start as `(path fingerprint, source_line)` or a small string derived from path + line number. Do not hash full message content on every render.

## Loading Strategy

Initial open should prioritize useful content quickly.

Recommended default:

- Load the tail first, because interrupted-session recovery usually needs the latest context.
- Parse enough lines from the end to show the latest 100-200 chat messages.
- Show a small loading affordance at the top: `Load older messages` / `Loading older...`.
- Continue loading older batches in the background only when requested or when the user scrolls near the top.

Implementation options:

- Phase 1 can read from the end in chunks by seeking backwards and splitting on newlines.
- If reverse JSONL reading is too risky for the first patch, use a bounded forward reader on a background task, but do not block the UI thread. The UI can render the first parsed batch immediately and continue appending.

Batch sizing:

- Parse batches by byte budget and line budget, for example `1-2 MiB` or `200 JSONL lines`, whichever comes first.
- Emit parsed chat-message batches to the UI after each batch.
- Keep invalid JSONL lines non-fatal; increment `parse_errors`.

Cancellation:

- Each transcript pane load gets a generation id.
- If the pane closes or opens another transcript, ignore stale batch events.
- Do not keep parsing in a task that can still mutate a dropped view.

## Parser Changes

Refactor current parsing into reusable line-level APIs:

```rust
pub fn parse_transcript_line(value: &Value) -> Option<TranscriptMessage>;
pub fn parse_transcript_jsonl_line(line: &str, source_line: usize) -> TranscriptParseLineResult;
pub fn parse_transcript_batch(lines: impl Iterator<Item = (usize, String)>) -> TranscriptBatch;
```

Keep current source-specific parsing:

- Codex `response_item` message payloads.
- Claude `message` objects.
- Generic fallback.

Keep current filtering:

- only `User` and `Assistant`;
- skip `<system-reminder>`;
- skip `# AGENTS.md instructions`;
- skip command XML wrappers;
- skip local command stdout/stderr wrappers.

Add tests for filtered input so markdown support does not accidentally bring raw tool/system content back into the pane.

## Markdown Rendering

Render Markdown only for visible messages, not during transcript parsing.

Pipeline:

1. The virtualized row asks for message `N`.
2. Renderer checks `markdown_cache` by message id.
3. On cache miss, parse `message.content` with `markdown_parser::parse_markdown`.
4. Convert parsed `FormattedText` into transcript-specific elements.
5. Store parsed/render model in an LRU cache.
6. If parsing fails or content is too large, fall back to plain text.

Supported blocks for first implementation:

- paragraphs / inline formatting;
- headings;
- unordered and ordered lists;
- task lists if parser output makes this straightforward;
- fenced code blocks in monospace blocks;
- horizontal rules;
- links as styled text if existing `FormattedTextElement` supports them cleanly;
- tables.

Explicitly disable or degrade:

- images: show link/source text, do not fetch/render images;
- Mermaid/HTML/embed blocks: render as fenced/plain fallback;
- huge single messages: render first capped chunk with `Show full message` later, not all at once.

## Table Rendering

Tables are mandatory.

Use `markdown_parser::FormattedTable` as the source representation:

- headers: `Vec<FormattedTextInline>`;
- alignments: `Vec<TableAlignment>`;
- rows: `Vec<Vec<FormattedTextInline>>`.

Do not render tables as raw pipe text except as fallback.

Preferred first implementation:

- Build a transcript-specific `render_markdown_table(table, app)` helper.
- Use a contained table block with:
  - header row;
  - row dividers;
  - cell padding;
  - theme-aware border/background;
  - horizontal scroll or clipping for wide tables;
  - max table height if rows are huge.
- Preserve alignment:
  - left/center/right from `TableAlignment`;
  - text wraps inside cells unless the cell is code-like.

For small/medium tables:

- Plain `Flex` grid is acceptable and simpler.
- Add a hard safety cap, for example render first `100` rows and show `N rows hidden` if exceeded.

For large tables:

- Use `warpui_core::elements::Table` with `TableStateHandle` if table rows are large enough to justify internal virtualization.
- This can be a second step after correctness, because nested virtualization inside a virtualized transcript row is more complex.

Acceptance criteria for tables:

- A Markdown table in a Claude/Codex answer renders as a real table.
- Wide tables do not push transcript pane content off-screen.
- Long cells wrap or clip predictably.
- The transcript pane remains scrollable after opening a message containing a large table.

## Virtualized Message List

Replace `ClippedScrollable(Flex::column(all messages))` with `viewported_list::List`.

Use:

- `ListState` stored on `SessionMemoryTranscriptView`;
- row count equal to `state.messages.len()` plus optional loader sentinel rows;
- row renderer builds only the requested message card;
- overscan handled by the existing list element.

Important details:

- Message cards have variable height, so use `viewported_list::List`, not `UniformList`.
- When a message's markdown parse result changes its height, invalidate that row height.
- Preserve scroll position while appending/prepending batches.
- For tail-first loading, opening a transcript should start near the bottom/latest loaded message.

Sentinel rows:

- Top sentinel: `Load older messages`, `Loading older...`, or `Start of transcript`.
- Bottom sentinel: `Loading newer...` only if forward streaming is active.
- Error sentinel: non-fatal parse/load error summary.

## Lazy Markdown Cache

Add a small cache to avoid reparsing visible messages every frame.

Cache key:

- message id;
- maybe content length/version if the same id could be reused.

Cache value:

```rust
enum ParsedMarkdownMessage {
    Markdown(FormattedText),
    PlainText(String),
    TooLarge { preview: String, original_len: usize },
}
```

Cache policy:

- LRU by message count, for example 200-500 parsed messages.
- Drop parsed entries when transcript path changes.
- Never cache raw JSON/tool events because they should not be rendered.

If Markdown parsing is visibly expensive for large messages, move markdown parsing to a low-priority background task for rows near the viewport and temporarily render plain text skeleton/preview. Start synchronous visible-row parsing first if it is fast enough after virtualization.

## Implementation Steps

1. Extract transcript parsing into a small model module.
   - Keep existing behavior with full-file parsing temporarily.
   - Add line-level and batch-level parser APIs.
   - Add tests for Claude, Codex, generic, filtering, and malformed JSONL.

2. Add markdown render helpers.
   - Parse `TranscriptMessage.content` through `markdown_parser`.
   - Render supported `FormattedTextLine` variants.
   - Add table renderer using `FormattedTable`.
   - Keep plain-text fallback for unsupported blocks and oversized messages.

3. Replace all-message `Flex::column` with `viewported_list::List`.
   - Store `ListState` on the transcript view.
   - Render rows by index.
   - Keep header outside the virtualized list.
   - Verify large synthetic transcript no longer builds all rows.

4. Add markdown cache.
   - Cache parsed markdown per message id.
   - Invalidate row height when a cached parse changes row layout.
   - Add caps for message size and table row count.

5. Move transcript loading off the UI thread.
   - Introduce `TranscriptLoadStatus`.
   - Parse in batches.
   - Emit batch events to append/prepend messages.
   - Show loading/error sentinel rows.

6. Add tail-first lazy loading.
   - Open transcript with latest messages first.
   - Add `Load older` top sentinel.
   - Preserve current scroll when prepending older messages.
   - Keep full forward-reader fallback if reverse reading is not ready.

7. Polish transcript UX.
   - Keep `Resume chat` in the pane header.
   - Add a small `Markdown`/`Plain` debug fallback only if needed during development; do not ship noisy controls by default.
   - Ensure table colors, code block backgrounds, and message cards match the Session Memory theme.

## Verification

Automated checks:

- `cargo test -p warp session_memory -- --nocapture`
- targeted tests for transcript parser batches;
- targeted tests for Markdown table parse/render model if the renderer is testable without screenshots;
- `cargo fmt --check`
- `cargo check -p warp`

Manual checks in Warp:

- Open a small Claude transcript: messages still filtered and readable.
- Open a small Codex transcript: AGENTS.md bootstrap hidden.
- Open a transcript with Markdown headings/lists/code.
- Open a transcript with a normal table.
- Open a transcript with a wide table.
- Open a large synthetic JSONL transcript; pane should open immediately and scrolling should remain responsive.
- Press `Resume chat` from transcript pane; command should still use the correct Claude/Codex resume form and preserve dangerous flags.

## Risks

- Nested scroll/virtualization: large tables inside a virtualized message list can fight with outer scrolling. Start with capped table rendering and horizontal scroll; use `warpui` table virtualization only if needed.
- Variable row heights: Markdown parse can change row height after first paint. Height invalidation must be explicit.
- Tail-first parsing: reverse JSONL reading is easy to get subtly wrong with partial UTF-8 boundaries. Keep a safe forward-reader fallback.
- Markdown parser feature flags: table parsing may depend on `FeatureFlag::MarkdownTables`; tests should force the expected flag state.

## Definition Of Done

- Transcript pane renders Markdown user/assistant messages.
- Markdown tables render as real tables, not raw pipe text.
- Large transcripts do not block pane opening.
- The view does not instantiate every message card on each render.
- Existing transcript filters and resume actions keep working.
