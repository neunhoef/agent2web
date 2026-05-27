# agent2web — Design Document

**Version:** 1.1  
**Status:** Revised — no auto-commit; selective commit page; working-tree diff

---

## 1. Overview

`agent2web` is a small, self-hosted Rust web server that acts as a
mobile-friendly control panel for a local AI coding agent (ForgeCode).
It closes the loop between a developer on the go and a running ForgeCode
instance on a home server:

1. Dictate a task via voice on a mobile browser.
2. Review and edit the transcribed text before dispatching.
3. The server runs ForgeCode non-interactively, resuming the current
   conversation or starting a new one.
4. When the agent finishes, the working-tree diff is available for
   review.
5. The user navigates to the commit page, selects the files to include,
   enters a subject line, and commits. The accumulated agent prompts are
   appended to the commit body automatically (formatted with `par`).
6. Repeat — each subsequent run continues the same ForgeCode
   conversation by default, preserving context across turns.

The agent **never commits automatically**. The developer stays in full
control of what goes into each commit and what the commit message says.

The entire UI is a server-rendered web application. JavaScript is used
only where it meaningfully improves the experience: audio capture, the
editable transcript field, and the live progress stream. Everything else
is plain HTML rendered on the server.

---

## 2. Goals and Non-Goals

### Goals

- Single-binary Rust server; no runtime dependencies beyond `git`,
  `forge`, `par`, and an STT provider.
- Fully usable on Android/iOS mobile browsers; also comfortable on
  desktop.
- Voice input with high STT quality; transcript is editable before
  submission.
- Live streaming of agent output so the user can follow progress.
- Beautiful, syntax-highlighted, multi-file HTML diff output via
  diff2html (rendered server-side into a full HTML page).
- **Working-tree diff** (`git diff HEAD`) showing all uncommitted changes
  at a glance after each agent run.
- Diff view supports both working-tree and commit-range review.
- Selective file commit: the user chooses exactly which modified files
  to stage and commit from a dedicated commit page.
- Commit body automatically includes all agent prompts since the last
  commit, formatted with `par` for readability.
- ForgeCode conversation continuity across multiple runs; explicit UI
  controls to start a new conversation.
- Minimal JavaScript — only where it makes a qualitative difference.

### Non-Goals

- Project configuration, credential management, or ForgeCode setup.
- Multi-user access control or authentication (assumed to run behind a
  VPN or SSH tunnel, or with HTTP basic auth added separately).
- GitHub / PR integration.
- Push notifications or background polling (the user watches live output
  or returns to the page).
- Windows or macOS support for the server (Linux target only).
- Automatic commits after agent runs.

---

## 3. Architecture

```
Mobile Browser
      │  HTTPS (reverse proxy)
      ▼
┌─────────────────────────────────────┐
│         agent2web (Rust binary)     │
│                                     │
│  ┌──────────┐   ┌─────────────────┐ │
│  │  Router  │──▶│ Handler: /      │ │  ← UI shell (HTML)
│  │ (axum)   │   │ Handler: /run   │ │  ← start agent run
│  │          │   │ Handler: /stream│ │  ← SSE live output
│  │          │   │ Handler: /diff  │ │  ← rendered diff HTML
│  │          │   │ Handler: /commit│ │  ← commit page + action
│  │          │   │ Handler: /audio │ │  ← STT upload endpoint
│  │          │   │ Handler: /conv  │ │  ← conversation management
│  └──────────┘   └─────────────────┘ │
│                         │           │
│              ┌──────────┘           │
│              ▼                      │
│  ┌───────────────────────────────┐  │
│  │        AppState (Arc)         │  │
│  │  run: Mutex<RunState>         │  │
│  │  conv: Mutex<ConvState>       │  │  ← active conversation ID + history
│  │  prompts_since_commit:        │  │  ← cleared on each git commit
│  │    Mutex<Vec<String>>         │  │
│  └───────────────────────────────┘  │
│              │                      │
│   ┌──────────┴──────────┐           │
│   ▼                     ▼           │
│  forge …            git operations  │
│  (subprocess)       (diff, commit)  │
└─────────────────────────────────────┘
              │
              ▼
     STT provider API
     (Whisper / Deepgram)
```

`agent2web` is a single process. It spawns subprocesses for `forge` and
`git`. There is no embedded database. All state is held in memory; if
the server restarts, the active conversation ID is lost (ForgeCode's
conversation history persists on disk in `~/.forge/`, so it can be
recovered manually if needed).

---

## 4. Conversation Model

### 4.1 How ForgeCode Manages Conversations

ForgeCode's ZSH plugin is the reference implementation for how
conversation state is managed in a non-interactive workflow.
Understanding it directly informs the `agent2web` design.

From the official ZSH docs:

> *"Prompts go to your last-used agent. If this is your first
interaction, it defaults to ForgeCode. The conversation continues across
prompts until you run `:new`."*

The ZSH plugin holds the active conversation ID as a **shell session
variable** — it persists for the lifetime of a terminal session.
Sending a bare `:` prompt always continues the current conversation.
`:new` clears the context and starts fresh. `:conversation` opens a
fuzzy picker to switch to any saved conversation; `:-` jumps back to the
previously active one.

This maps cleanly onto `agent2web`: the server process is the "session",
the in-memory `ConvState` plays the role of the shell session variable,
and the UI provides the equivalent of `:new` and `:conversation` as
explicit buttons.

### 4.2 ForgeCode CLI Primitives

```sh
# Run a prompt, continuing the most recently active conversation:
forge -p "<prompt>"

# Allocate a new conversation ID (printed to stdout):
forge conversation new

# Open an existing conversation in interactive TUI mode (not useful for us):
forge conversation resume <id>   # NOTE: opens TUI, not scriptable as-is

# Inspect and manage:
forge conversation list           # list all saved conversations
forge conversation info <id>      # show metadata (created, tokens, label)
forge conversation stats <id>     # show token usage
forge conversation show <id>      # print last assistant message
forge conversation dump <id>      # export full conversation as JSON
forge conversation compact <id>   # compact context to reduce token usage
forge conversation rename <id> <name>
forge conversation delete <id>
```

**Key insight from the ZSH plugin source:** `forge -p "<prompt>"`
always continues the most recently active conversation on disk —
it does not require passing a conversation ID explicitly. ForgeCode
tracks the "current conversation" as part of its own persistent state
in `~/.forge/`. The ZSH plugin stores the active ID in a shell variable
only for its own bookkeeping (e.g. display in RPROMPT, enabling `:-` to
jump back). The `forge` binary itself picks up the last conversation
automatically.

**Consequence for `agent2web`:** Starting a new conversation requires
`forge conversation new` to obtain a fresh ID, followed immediately by
`forge -p "<prompt>"`. ForgeCode will then use that new conversation for
subsequent runs. There is no need for a `--conversation-id` flag passed
on every invocation; the server only needs to call `forge conversation
new` when the user explicitly requests a fresh start. Normal "resume"
behaviour is the default.

### 4.3 ConvState

The server tracks conversation state in `ConvState`, protected by a
`Mutex`:

```rust
pub struct ConvState {
    /// The ForgeCode conversation ID currently active.
    /// None only before the very first run ever. After the first run, always set.
    pub active_id: Option<String>,
    /// Ordered list of all conversation entries, most recent last.
    pub history: Vec<ConvEntry>,
}

pub struct ConvEntry {
    pub id: String,
    pub started_at: SystemTime,
    /// Label: first prompt of the conversation, truncated, for display.
    pub label: String,
    /// Number of runs completed under this conversation.
    pub run_count: u32,
}
```

On server start, `active_id` is `None`. The first `POST /run` implicitly
creates a ForgeCode conversation (by calling `forge -p` and then
reading the new ID via `forge conversation list`). Subsequent runs call
`forge -p` directly — ForgeCode resumes the last active conversation
automatically. `active_id` is updated whenever `forge conversation new`
is invoked.

### 4.4 The Run Invocation Pattern

```
New conversation (POST /conversation/new followed by the next POST /run):
    forge conversation new          → prints new_id to stdout
    store new_id in ConvState
    forge -p "<prompt>"             → ForgeCode uses new_id automatically

Normal resume (every subsequent POST /run):
    forge -p "<prompt>"             → ForgeCode continues active_id automatically
    increment ConvEntry::run_count
```

No flags are required on the normal path. The "new conversation" path
only requires `forge conversation new` to obtain the ID for our own
bookkeeping; ForgeCode handles the rest.

### 4.5 Starting a New Conversation

`POST /conversation/new` does the following:

1. Runs `forge conversation new`, capturing the printed ID from stdout.
2. Stores it as `ConvState::active_id`.
3. Appends a new `ConvEntry` to `history` (label set from the first
   prompt of the next run).
4. Returns `303 See Other` → `/`.

The old conversation ID remains in `history`, resumable via `POST
/conversation/resume`.

---

## 5. HTTP API

All routes return HTML unless noted. The UI shell is a single HTML page;
partial updates use HTMX for the progress stream and diff swap, avoiding
a full page reload while staying within the no-JS-framework constraint.

> **On HTMX:** HTMX is a ~14 kB library that enables server-driven
partial page updates via standard HTML attributes. It is the one JS
dependency that provides genuine value here: it allows the agent output
stream and diff replacement to work without writing any custom JS. All
other interactivity (audio recording, editable transcript) requires a
small amount of hand-written JS, described in §8.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/` | Main UI page |
| `POST` | `/audio` | Upload audio blob → returns transcript JSON |
| `POST` | `/run` | Submit prompt text → starts agent run, returns 303 redirect to `/` |
| `GET` | `/stream` | SSE endpoint; streams agent stdout/stderr |
| `GET` | `/diff` | Working-tree diff (`git diff HEAD`) rendered via diff2html |
| `GET` | `/diff/commit?ref=REF` | Diff introduced by a single commit (default: `HEAD`) |
| `GET` | `/diff/range?from=SHA&to=SHA` | Diff between two arbitrary commits |
| `POST` | `/conversation/new` | Reset active conversation; next run starts fresh |
| `GET` | `/conversation/list` | HTML fragment listing conversation history (HTMX swap target) |
| `POST` | `/conversation/resume` | Resume a specific past conversation by ID (form field: `id`) |
| `GET` | `/commit` | Commit page: file selector, subject input, prompt preview |
| `POST` | `/commit` | Execute the commit with selected files and subject |
| `GET` | `/health` | `200 OK` plain text — for uptime monitoring |

### 5.1 `GET /`

Returns the full UI shell (see §7). Includes current run status and
conversation status reflected in the page. If a run is in progress, the
page auto-connects to `/stream`. After a run completes, a **[ Review &
Commit ]** link appears pointing to `GET /commit`, and a **[ View Diff ]**
link points to `GET /diff` to show the working-tree diff.

### 5.2 `POST /audio`

- Content-Type: `multipart/form-data`, field name `audio`, blob format
  `webm/opus` (recorded by the browser) or `wav`.
- Server writes the blob to a temp file, calls the configured STT
  provider, returns:
```json
{ "transcript": "refactor the parser to handle edge cases" }
```
- Errors return `{ "error": "…" }` with an appropriate HTTP status.
- The transcript is injected by a small JS snippet into the editable
  `<textarea>` on the page.

### 5.3 `POST /run`

- Form fields: `prompt` (text string), `password` (optional, see §14).
- Server validates: non-empty prompt, no run already in progress.
- Spawns the agent subprocess asynchronously (see §6).
- Returns `303 See Other` → `/` so the browser reloads, showing the
  running state.

### 5.4 `GET /stream`

- Server-Sent Events (SSE) stream.
- Each event is one line of agent stdout/stderr:
  ```
  data: [forge] Analyzing codebase…\n\n
  data: [forge] Writing patch to src/parser.rs…\n\n
  data: __DONE__\n\n
  ```
- The `__DONE__` sentinel causes the client JS to reload the page, which
  then shows the completed run status with links to the diff and commit
  page.
- The stream stays open until the run completes or the client
  disconnects.

### 5.5 `GET /diff`, `GET /diff/commit`, and `GET /diff/range`

Three diff modes are available, all rendered via diff2html-cli (see §9):

**`GET /diff`** — working-tree diff (all uncommitted changes):
- Runs `git diff HEAD` in `project_dir`.
- This is the primary diff view after an agent run, showing everything
  that changed since the last commit.
- If the working tree is clean, returns a page indicating no changes.

**`GET /diff/commit?ref=REF`** — single-commit diff:
- `ref` defaults to `HEAD` if absent (shows the most recent commit).
- Runs `git diff <ref>~1..<ref>`.
- Useful for reviewing what was introduced by a specific past commit.

**`GET /diff/range?from=SHA&to=SHA`** — range diff:
- `from` defaults to `HEAD~1`, `to` defaults to `HEAD`.
- Runs `git diff <from>..<to>`.
- Useful for cumulative review across several commits.

All three return a complete standalone HTML page (the diff2html output).
The user navigates to the diff page and uses the browser Back button to
return to the main UI.

### 5.6 `POST /conversation/new`

- Optional form field: `label` (short description of the new work,
  stored in `ConvEntry`).
- Runs `forge conversation new`, stores the returned ID as
  `ConvState::active_id`, and appends a new `ConvEntry` to `history`.
- Returns `303 See Other` → `/`.
- The UI reflects the new conversation ID in the status bar.

### 5.7 `GET /conversation/list`

- Returns an HTML fragment listing all `ConvEntry` items in
  `ConvState::history`.
- Each entry shows: ID (truncated), label, started_at, run_count, and a
  "Resume" button that posts to `/conversation/resume`.
- Intended as an HTMX swap target, but also usable standalone.

### 5.8 `POST /conversation/resume`

- Form field: `id` (ForgeCode conversation ID).
- Validates that the ID exists in `ConvState::history`.
- Sets `ConvState::active_id = Some(id)`.
- Returns `303 See Other` → `/`.

### 5.9 `GET /commit` — Commit Page

Returns a full standalone HTML page with:

1. **File selector** — runs `git status --porcelain` to enumerate all
   modified, added, deleted, and renamed files in the working tree.
   Each file is shown as a checkbox row (checked by default). The user
   can deselect files they do not want to include in this commit.

2. **Diff link per file** — each file row optionally links to
   `GET /diff` filtered to that path (future enhancement; for now a
   single "View full diff" link points to `GET /diff`).

3. **Subject input** — a single-line `<input>` for the commit subject
   line (required).

4. **Prompt preview** — a read-only list of all prompts accumulated in
   `AppState::prompts_since_commit` since the last commit, so the user
   can confirm what context will be included in the commit body.

5. **[ Commit Selected Files ]** button — submits `POST /commit`.

If there are no changed files, the page shows a notice and the commit
button is absent.

```
┌────────────────────────────────────────────────────────────┐
│  agent2web  [project: my-proj]   Password: [__]            │
├────────────────────────────────────────────────────────────┤
│  Commit Changes                           ← Back to Home   │
│                                                            │
│  Changed files (select to include):                        │
│  ☑ M  src/main.rs                                          │
│  ☑ M  src/agent.rs                                         │
│  ☐ ?  scratch.txt                                          │
│                                                            │
│  Subject: [_______________________________]                │
│                                                            │
│  Prompts to be appended to commit body:                    │
│  · refactor the parser to handle edge cases                │
│  · add unit tests for the new token types                  │
│                                                            │
│  [ Commit Selected Files ]   [ View Diff ]                 │
└────────────────────────────────────────────────────────────┘
```

### 5.10 `POST /commit` — Execute Commit

- Form fields: `message` (commit subject, required), `files[]` (zero or
  more selected file paths), `password`.
- Validates password (see §14).
- Rejects if a run is currently in progress (409 Conflict).
- Rejects if `files[]` is empty or `message` is blank (400 Bad Request).
- Stages **only** the selected files:
  ```sh
  git add -- <file1> <file2> …
  ```
- Builds the commit message (see §5.10.1 below).
- Runs `git commit -F -` passing the message via stdin (avoids shell
  quoting issues with multi-line messages).
- On success: clears `AppState::prompts_since_commit` and returns `303
  See Other` → `/`.
- On failure: returns an error page with the git output.

#### 5.10.1 Commit Message Format

The commit message is:

```
<subject line>

Prompts since last commit:

<formatted prompt body>
```

The `<formatted prompt body>` is built by:
1. Joining all prompts with a blank line between each.
2. Piping the result through `par 72` for line-wrapping to 72 columns.

Example with two prompts:

```
refactor the parser to handle edge cases

Prompts since last commit:

refactor the parser to handle edge cases

add unit tests for the new token types
```

After wrapping, long prompts are reflowed to 72 columns. `par` is
expected to be on `$PATH`; if it is absent, the body is included
without reflowing (plain concatenation).

#### 5.10.2 File Path Security

File paths submitted as `files[]` are validated against the list
returned by `git status --porcelain` at request time. Any path not in
that set is rejected (400 Bad Request). This prevents path-traversal
attacks and ensures only files git knows about can be staged.

---

## 6. Agent Run Lifecycle

```
POST /run (prompt)
    │
    ├─ Reject if run already in progress → 409 Conflict
    │
    ├─ Read ConvState::active_id
    │
    ├─ Set RunState { status: Running, output_buf: vec![] }
    │
    ├─ Append prompt to prompts_since_commit
    │
    ├─ Spawn tokio task:
    │       │
    │       ├─ Run: forge -p "<prompt>"
    │       │       (ForgeCode automatically continues the last active
    │       │        conversation on disk — no ID flag needed)
    │       │
    │       ├─ If active_id is None (first ever run):
    │       │       Run: forge conversation list
    │       │       Capture new conversation ID from output
    │       │       Set ConvState::active_id = Some(new_id)
    │       │       Append ConvEntry to ConvState::history
    │       │
    │       ├─ Else: increment ConvEntry::run_count
    │       │
    │       ├─ Stream stdout+stderr into output_buf
    │       │       (broadcast on SSE channel)
    │       │
    │       ├─ On exit code 0:
    │       │       Set RunState { status: Done }
    │       │       (no automatic commit)
    │       │
    │       └─ On non-zero exit:
    │               Set RunState { status: Failed(reason) }
    │
    └─ Return 303 → /
```

After the run completes the working tree will have uncommitted changes.
The user navigates to `GET /diff` to review them and to `GET /commit`
to stage and commit the files they want.

`RunState` and `ConvState` are both fields on `AppState`, wrapped in
`Arc`. Each is protected by its own `Mutex`, held only for reads/writes
to the struct, never during subprocess execution.

### 6.1 Concurrency

Only one run is permitted at a time. A second `POST /run` while a run is
active returns `409 Conflict` with a human-readable error page.

### 6.2 Timeouts

A configurable `run_timeout` (default: 600 seconds) kills the subprocess
if it exceeds the limit and transitions the run to `Failed(timeout)`.

---

## 7. UI Structure

### 7.1 Main Page (`GET /`)

The main page has five logical sections, rendered top-to-bottom:

```
┌─────────────────────────────────────────────────┐
│  agent2web   [project: my-proj]   Password: [__]│  ← header + password field
│  Conversation: abc123 (3 runs)  [New Conv ▶]    │  ← conversation status bar
├─────────────────────────────────────────────────┤
│  [ Record ]  [ Stop ]                           │  ← voice capture controls
│  ┌─────────────────────────────────────────┐    │
│  │ Editable transcript textarea            │    │
│  └─────────────────────────────────────────┘    │
│  [ Send to Agent ]                              │  ← submit button
├─────────────────────────────────────────────────┤
│  Agent Output                                   │  ← live stream area
│  ┌─────────────────────────────────────────┐    │
│  │ [forge] Analyzing codebase…             │    │
│  │ [forge] Writing src/parser.rs…          │    │
│  └─────────────────────────────────────────┘    │
├─────────────────────────────────────────────────┤
│  [ View Working-Tree Diff ]  [ Review & Commit ]│  ← action links
└─────────────────────────────────────────────────┘
```

### 7.2 Password Field

A compact `<input type="password" id="password">` field is placed in the
page header, persistent across all interactions. A small JS snippet (~15
lines) reads this field's value and injects it as a hidden `password`
input into every action form immediately before submission (`submit`
event handler). For the JS-driven audio upload (`POST /audio`), the
password is appended to the `FormData` object. The value is also saved
to and restored from `sessionStorage` so the user need not re-enter it
after a page reload. The field is always visible — there is no login
page or session cookie.

If the server has no password configured (`password = ""`), the field is
hidden and no validation is performed server-side.

### 7.3 Conversation Status Bar

A persistent bar beneath the header shows:
- The active conversation ID (truncated to 8 chars) and its label if
  set.
- The run count for the current conversation.
- A **[ New Conversation ]** button (posts to `/conversation/new`),
  which opens an optional inline form for a label before confirming.
- A **[ History ]** toggle that expands a panel rendered via HTMX from
  `/conversation/list`, showing past conversations with resume buttons.

When `active_id` is `None`, the bar shows: *"No active conversation —
next run will start a new one."*

### 7.4 Action Links

A row of action links beneath the agent output area:

- **[ View Working-Tree Diff ]** — links to `GET /diff`, opening the
  working-tree diff page in a new or current tab.
- **[ Review & Commit ]** — links to `GET /commit`, opening the commit
  page where files can be selected and a commit created.

These links are always visible. The commit page will simply show "no
changed files" if the working tree is clean.

### 7.5 Responsiveness

- Layout uses CSS Flexbox/Grid with a single-column mobile layout
  (≤768px) and a two-column option on desktop (prompt+output left,
  action links right).
- Font sizes and tap targets are sized for mobile-first (minimum 44px
  tap target height per WCAG).
- No external CSS frameworks. Custom CSS is inlined into the HTML shell
  at build time via `include_str!()`.

### 7.6 Page States

The main page reflects the server's `RunState` and `ConvState` at render
time:

| State | UI Behavior |
|-------|-------------|
| `Idle, no conversation` | Prompt area enabled; conversation bar shows "no active conversation" |
| `Idle, conversation active` | Prompt area enabled; conversation bar shows ID and run count |
| `Running` | Prompt area disabled; output stream area visible and auto-scrolling; SSE connected |
| `Done` | Output area shows completion notice; action links prominent |
| `Failed(reason)` | Error message shown; prompt area re-enabled |

---

## 8. JavaScript Usage

The following are the only JavaScript components. All are hand-written
(no frameworks, no build step, no bundler). Total JS budget: ≤ 300
lines.

### 8.1 Audio Capture (`~80 lines`)

The Web Speech API's `SpeechRecognition` interface has inconsistent
support and poor quality on mobile. Instead, audio is recorded as a blob
and uploaded to the server for high-quality transcription.

```js
// Pseudocode outline
const recorder = new MediaRecorder(stream, { mimeType: 'audio/webm;codecs=opus' });
recorder.ondataavailable = e => chunks.push(e.data);
recorder.onstop = async () => {
  const blob = new Blob(chunks, { type: 'audio/webm' });
  const form = new FormData();
  form.append('audio', blob, 'recording.webm');
  const res = await fetch('/audio', { method: 'POST', body: form });
  const { transcript } = await res.json();
  document.getElementById('prompt').value = transcript;
};
```

- Record button triggers `getUserMedia` + `MediaRecorder.start()`.
- Stop button triggers `MediaRecorder.stop()`, which fires `onstop`.
- The transcript is placed into the `<textarea>` for review and editing.
- Error states (microphone denied, upload failed) are shown inline.

### 8.2 SSE Live Output (`~40 lines`)

When the page loads in `Running` state (detected via the
`data-sse-running` attribute on the output `<div>`), a plain
`EventSource` connects to `/stream`. Each `message` event appends a
line to the output box. The `__DONE__` sentinel causes the client to
close the connection and reload the page.

No HTMX is used for SSE; the hand-written JS handles all of it.

### 8.3 Auto-scroll (`~10 lines`)

The agent output `<div>` auto-scrolls to the bottom as new lines arrive.
A `MutationObserver` on the output container handles this in ~10 lines.

### 8.4 What Is Not JavaScript

- Page layout and responsiveness — pure CSS.
- Form submission (run, commit, new conversation, resume conversation)
  — native HTML `<form method="POST">`.
- Diff rendering — done entirely server-side.
- Syntax highlighting — embedded in the diff2html-generated HTML.
- State management — reflected in server-rendered HTML on each full
  page load.
- File selection on the commit page — native HTML checkboxes.

---

## 9. Diff Rendering

### 9.1 Approach

The server invokes `git diff` as a subprocess and pipes the unified
diff text through `diff2html-cli`. This is an explicit design
choice: diff2html-cli produces GitHub-quality output with correct
syntax highlighting, word-level matching, sticky file headers, and a
collapsible file list. The Node.js runtime dependency is accepted;
`diff2html-cli` is installed once (`npm install -g diff2html-cli`)
alongside `forge` and `git`.

A future migration to a pure Rust renderer (using the `similar`
crate) remains possible — the diff rendering logic is isolated in
`src/diff.rs` behind a trait — but is not a priority.

### 9.2 Invocation

**Working-tree diff** (`GET /diff`):

```sh
git diff HEAD \
  | diff2html --style line \
              --syntax-highlight \
              --file-list-toggle \
              -i stdin \
              -o stdout
```

**Single-commit diff** (`GET /diff/commit?ref=REF`):

```sh
git diff <ref>~1..<ref> | diff2html …
```

**Range diff** (`GET /diff/range?from=SHA&to=SHA`):

```sh
git diff <from_sha>..<to_sha> | diff2html …
```

All three modes return a complete standalone HTML page (the diff2html
output is self-contained). The server renders this directly; no wrapper
template is needed. The user navigates to the diff page and uses the
browser Back button to return.

### 9.3 Standalone Diff Page

Each `GET /diff*` endpoint returns a complete standalone HTML page.
Bookmarking a specific commit diff or sharing it within a local network
is straightforward.

### 9.4 Diff History (future)

The server may maintain an in-memory list of recent commit SHAs (up to a
configurable limit, default 20). The UI could render these as quick-jump
buttons on the diff page, allowing the user to load any recent commit
diff without knowing the SHA.

---

## 10. Speech-to-Text

### 10.1 Provider Strategy

The browser records audio as `webm/opus` and POSTs it to `/audio`.
The server transcribes it locally via a persistent `whisper-server`
process. A remote API fallback is supported for convenience but is not
the recommended path.

| Provider | Quality | Latency (short clip) | Cost | Notes |
|----------|---------|----------------------|------|-------|
| **`whisper-server` (local, CUDA)** | Excellent | **~200–500 ms warm** | Free | **Recommended default.** Persistent HTTP server; model stays in GPU VRAM between requests. |
| **OpenAI Whisper API** | Excellent | ~1–3s + network | $0.006/min | Convenient fallback; negligible cost for dictation clips (~$0.003 per 30s clip). |
| **Deepgram Nova-3** | Excellent | <1s + network | ~$0.004/min | Slightly cheaper remote option; strong on technical vocabulary. |
| **Web Speech API** | Variable | Instant | Free | Not used — quality too inconsistent on mobile for code-related terms. |

The provider interface in Rust is a trait:

```rust
#[async_trait]
pub trait SttProvider: Send + Sync {
    async fn transcribe(&self, audio: Bytes, mime_type: &str) -> Result<String>;
}
```

Concrete implementations: `WhisperServerProvider` (local HTTP),
`WhisperApiProvider` (OpenAI), `DeepgramProvider`.

### 10.2 Local whisper-server Setup

`whisper.cpp` ships a built-in HTTP server (`whisper-server`) that
loads the model once into GPU VRAM and serves subsequent requests with
near-zero overhead. This is the correct integration pattern — **not**
spawning `whisper-cli` as a subprocess per request, which pays the full
model-load penalty (~2–4s) on every invocation.

**Build with CUDA support:**

```sh
git clone https://github.com/ggml-org/whisper.cpp
cd whisper.cpp
cmake -B build -DGGML_CUDA=1
cmake --build build -j --config Release

# Download large-v3-turbo weights (~1.6 GB, 4-bit quantized variant ~800 MB)
bash ./models/download-ggml-model.sh large-v3-turbo
# or the quantized Q5_0 variant for slightly lower VRAM at near-identical accuracy:
# bash ./models/download-ggml-model.sh large-v3-turbo-q5_0
```

**Run as a persistent service:**

```sh
./build/bin/whisper-server   --model models/ggml-large-v3-turbo.bin   --host 127.0.0.1   --port 8090   --inference-path /v1/audio/transcriptions   --threads 4   --processors 1
```

The `--inference-path /v1/audio/transcriptions` flag makes the endpoint
OpenAI API-compatible, so the Rust client code is identical regardless
of whether it targets the local server or OpenAI's cloud.

**Model and hardware fit:**

- `large-v3-turbo` uses ~3 GB VRAM at FP16, well within the 16 GB
  available. The remaining VRAM is free for other processes.
- For short dictation clips (5–30 seconds), warm inference on a
  mid-range NVIDIA GPU takes **~150–500 ms** — fast enough to feel
  instant.
- `large-v3-turbo` has 4 decoder layers instead of 32 (vs. full
  large-v3), giving ~48% faster decoding at minimal accuracy cost — the
  right choice here since speed matters and clips are short and clean.
- If maximum accuracy is preferred over speed, `large-v3` (~3 GB VRAM,
  same fit) can be substituted with no other changes.

**Run as a systemd service** so it starts automatically and is always
warm:

```ini
[Unit]
Description=whisper-server STT
After=network.target

[Service]
ExecStart=/path/to/whisper-server   --model /path/to/ggml-large-v3-turbo.bin   --host 127.0.0.1 --port 8090   --inference-path /v1/audio/transcriptions
Restart=always
Environment=CUDA_VISIBLE_DEVICES=0

[Install]
WantedBy=multi-user.target
```

### 10.3 Configuration

```toml
[stt]
provider = "whisper_server"   # or "openai", "deepgram"

[stt.whisper_server]
url   = "http://127.0.0.1:8090/v1/audio/transcriptions"
# model field is informational only (server decides which model to use)

[stt.openai]
api_key = ""   # override with AGENT2WEB_STT_API_KEY
model   = "whisper-1"

[stt.deepgram]
api_key = ""   # override with AGENT2WEB_STT_DEEPGRAM_KEY
```

### 10.4 Transcript Review

After the audio upload returns the transcript, it is placed into a
`<textarea>`. The user can:
- Read it through and correct any mis-transcriptions (common for
  technical terms, identifiers, file names).
- Edit freely — the textarea is a standard HTML editable field.
- Submit when satisfied, or clear and re-record.

The submit button is disabled while audio is uploading or a run is in
progress.

### 10.5 Audio Format

The browser records `webm/opus`. `whisper-server` expects 16 kHz mono
WAV (16-bit PCM). The Rust handler converts the upload using `ffmpeg`
before forwarding:

```sh
ffmpeg -i input.webm -ar 16000 -ac 1 -f wav output.wav
```

`ffmpeg` is already a common dependency on Linux servers; it is added to
the list of external runtime dependencies.

---

## 11. Configuration

Configuration is read from a TOML file (default: `./agent2web.toml`)
with environment variable overrides for secrets. No CLI argument parsing
beyond `--config <path>`.

```toml
[server]
bind         = "0.0.0.0:8080"
project_dir  = "/home/user/myproject"   # the git repo ForgeCode operates on
run_timeout  = 600                      # seconds
password     = ""                       # shared password for all action endpoints;
                                        # leave empty to disable auth entirely.
                                        # Override with AGENT2WEB_PASSWORD env var.

[server.tls]
enabled      = false
cert         = "tls/cert.pem"           # path to PEM certificate (absolute or
key          = "tls/key.pem"            # relative to the config file location)

[forge]
binary       = "forge"                  # path or name on $PATH

[stt]
provider     = "whisper_api"
api_key      = ""                       # override with AGENT2WEB_STT_API_KEY

[diff]
max_history  = 20                       # number of recent commits to track
context_lines = 5                       # lines of context in diff output
```

> **Note:** The `commit_cmd` and `auto_push` fields from earlier versions
> are removed. The server never commits automatically; all commits are
> initiated by the user from the commit page.

---

## 12. Rust Crate Structure

```
agent2web/
├── src/
│   ├── main.rs          — binary entry point, config loading, server startup
│   ├── config.rs        — Config struct, TOML deserialization
│   ├── router.rs        — axum Router, all route registrations
│   ├── state.rs         — AppState, RunState, ConvState, ConvEntry
│   ├── auth.rs          — password middleware: validates `password` field on
│   │                      all mutating requests; no-op when password unconfigured
│   ├── handlers/
│   │   ├── mod.rs
│   │   ├── index.rs     — GET /
│   │   ├── run.rs       — POST /run
│   │   ├── stream.rs    — GET /stream (SSE)
│   │   ├── diff.rs      — GET /diff, GET /diff/commit, GET /diff/range
│   │   ├── audio.rs     — POST /audio
│   │   ├── commit.rs    — GET /commit (page), POST /commit (action)
│   │   └── conversation.rs  — POST /conversation/new, GET /conversation/list,
│   │                          POST /conversation/resume
│   ├── agent.rs         — ForgeCode subprocess management, conversation ID
│   │                      detection, resume adapter
│   ├── stt/
│   │   ├── mod.rs       — SttProvider trait
│   │   ├── whisper_api.rs
│   │   ├── deepgram.rs
│   │   └── whisper_cpp.rs
│   ├── diff.rs          — git diff invocation, diff2html-cli bridge
│   └── templates.rs     — HTML template functions (server-side rendering)
├── static/
│   ├── style.css        — mobile-first CSS (inlined at build time)
│   └── app.js           — hand-written JS (inlined at build time)
├── tls/
│   ├── generate.sh      — one-shot script to create a self-signed cert+key
│   ├── cert.pem         — (git-ignored; generated by generate.sh)
│   └── key.pem          — (git-ignored; generated by generate.sh)
└── agent2web.toml.example
```

Static assets (`style.css`, `app.js`) are embedded into the binary at
compile time using `include_str!()`, so the deployed artifact is a
single self-contained binary.

---

## 13. Key Dependencies

| Crate | Purpose |
|-------|---------|
| `axum` | HTTP server, routing, SSE, multipart |
| `tokio` | Async runtime |
| `tokio::process` | Subprocess execution (forge, git, diff2html) |
| `serde` + `serde_json` | JSON for STT responses and config |
| `toml` | Config file parsing |
| `reqwest` | HTTP client for STT API calls |
| `bytes` | Audio blob handling |
| `tracing` + `tracing-subscriber` | Structured logging |
| `thiserror` | Error types |
| `async-trait` | Async trait for SttProvider |
| `axum-server` (feature `tls-rustls`) | TLS listener for axum; uses `rustls` under the hood |
| `rustls` + `rustls-pemfile` | TLS implementation and PEM parsing |

External runtime dependencies:
- `forge` (ForgeCode binary)
- `git`
- `par` (paragraph formatter; used to reflow the commit body. Install
  via your package manager: `apt install par` or `brew install par`)
- `node` + `diff2html-cli` (`npm install -g diff2html-cli`)
- `whisper-server` (built from `whisper.cpp` with `-DGGML_CUDA=1`; runs as a persistent local service)
- `ffmpeg` (audio format conversion: `webm/opus` → 16 kHz WAV)

---

## 14. Security Considerations

`agent2web` executes shell commands with user-supplied prompt text. The
following mitigations apply:

- The prompt is passed to `forge` as a single argument, not
  interpolated into a shell string. Subprocesses are spawned via
  `tokio::process::Command` with explicit argument lists, never via `sh
  -c`.
- Conversation IDs are validated against `ConvState::history` before
  being passed to `forge`; arbitrary strings are rejected.
- File paths submitted to `POST /commit` are validated against the
  current `git status --porcelain` output; only known changed files can
  be staged.
- The `project_dir` is configured at startup and is not
  user-controllable at runtime.
- Audio uploads are size-limited (configurable, default 25 MB) to
  prevent local disk exhaustion.
- SSE connections are limited to one per run; a new connection drops the
  previous one.

### 14.1 Password Authentication

All endpoints that produce a server-side action (`POST /run`,
`POST /audio`, `POST /commit`, `POST /conversation/new`, `POST
/conversation/resume`) require a `password` form field. The server
compares it to the value of `server.password` in the configuration (or
the `AGENT2WEB_PASSWORD` environment variable). A mismatch returns `403
Forbidden`. Read-only endpoints (`GET /`, `GET /stream`, `GET /diff*`,
`GET /commit`, `GET /conversation/list`, `GET /health`) are not
password-protected.

When `server.password` is empty (the default), authentication is
disabled entirely — no password field is required or checked.

The password is intentionally simple (no hashing, no sessions, no
tokens). It is a shared secret transmitted over the encrypted connection
and is sufficient for a single-user self-hosted deployment. The UI
stores the value in `sessionStorage` and injects it automatically into
every form.

### 14.2 TLS

When `server.tls.enabled = true`, the server binds a TLS listener
using `axum-server` with `rustls`. The certificate and private key are
read from the paths given by `server.tls.cert` and `server.tls.key`. A
self-signed certificate is sufficient for personal use (see §17 for the
generation script). Combined with the password, this provides reasonable
security for a personal tool exposed over a home network or VPN without
requiring a reverse proxy.

When TLS is disabled the server binds a plain HTTP listener, which is
appropriate when sitting behind a TLS-terminating reverse proxy such as
nginx or Caddy.

---

## 15. Development Milestones

| Milestone | Scope |
|-----------|-------|
| **M1 — Skeleton** | Server boots, serves static HTML shell, `/health` works |
| **M2 — Diff View** | `GET /diff` renders a real commit diff via diff2html; multi-file, syntax-highlighted |
| **M3 — Agent Run** | `POST /run` spawns `forge -p`, streams output via SSE; no auto-commit; new conversation detected automatically |
| **M4 — Commit Page** | `GET /commit` file-selector page; `POST /commit` stages selected files, builds commit message with `par`-formatted prompt body; `GET /diff` updated to show working-tree diff; `GET /diff/commit` added |
| **M5 — Conversation Management** | `ConvState` tracking, `/conversation/new`, `/conversation/list`, `/conversation/resume`; conversation status bar in UI |
| **M6 — Voice Input** | Audio capture in browser, upload to `/audio`, Whisper API transcription, editable textarea |
| **M7 — Polish** | Mobile layout refinement, diff history nav, error states, timeouts |
| **M8 — Offline STT** | `whisper_cpp` provider implementation for fully local operation |
| **M9 — Security** | Password authentication middleware; optional TLS listener with `axum-server`/`rustls`; `tls/generate.sh` script |

---

## 16. Open Questions

1. **ForgeCode conversation invocation — resolved:** Research into
    the ZSH plugin source and official docs confirms the correct pattern.
    `forge -p "<prompt>"` automatically continues the most recently
    active conversation on disk; no `--conversation-id` flag is needed or
    exists. The only special case is starting a new conversation: `forge
    conversation new` (which prints the new ID to stdout) must be called
    first, so the server can record the ID for its own bookkeeping. `forge
    conversation resume <id>` opens the interactive TUI and is not usable
    non-interactively. The adapter in `src/agent.rs` is therefore simple and
    well-defined.

2. **Auto-push after commit:** The commit page could offer an optional
    "Push after commit" checkbox. Currently there is no auto-push; the
    user would need to push from a terminal. A future milestone could add
    a `POST /push` endpoint or a checkbox on the commit page.

3. **Multi-project support:** Currently one `project_dir` per server
    instance. Running multiple instances on different ports is the simplest
    solution. A project switcher in the UI is a possible future addition.

4. **Conversation persistence across server restart:** Currently
    `ConvState` is in-memory only. A simple on-disk JSON file written after
    each state change would survive restarts with minimal added complexity,
    and would allow the user to resume conversations even after rebooting
    the server.

5. **Streaming diff updates:** Currently the diff is loaded once
    after the user navigates to the diff page. For very long-running agents
    that modify many files, streaming intermediate diffs could be valuable.

6. **`par` availability:** If `par` is not installed, the commit body
    is included without reflowing. A future improvement could bundle a
    simple Rust line-wrapper as a fallback so there is no hard dependency.

---

## 17. TLS Setup

When `server.tls.enabled = true`, a certificate and private key must be
present before starting the server. The `tls/generate.sh` script creates
a 10-year self-signed RSA-4096 certificate suitable for personal use:

```sh
#!/usr/bin/env bash
# tls/generate.sh — generate a self-signed TLS certificate for agent2web
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")"; pwd)"

openssl req \
  -x509 \
  -newkey rsa:4096 \
  -keyout "${SCRIPT_DIR}/key.pem" \
  -out    "${SCRIPT_DIR}/cert.pem" \
  -sha256 \
  -days   3650 \
  -nodes \
  -subj   "/CN=agent2web" \
  -addext "subjectAltName=IP:127.0.0.1,IP:::1,DNS:localhost"

echo "Done."
echo "  Certificate: ${SCRIPT_DIR}/cert.pem"
echo "  Private key: ${SCRIPT_DIR}/key.pem"
echo
echo "Add the following to agent2web.toml:"
echo "  [server.tls]"
echo "  enabled = true"
echo "  cert    = \"tls/cert.pem\""
echo "  key     = \"tls/key.pem\""
```

Run it once from the repository root:

```sh
bash tls/generate.sh
```

Both `tls/cert.pem` and `tls/key.pem` should be listed in `.gitignore`.
The certificate is self-signed: browsers will show a certificate warning
on first access. Accept it once (or install the cert as a trusted CA on
your devices) and subsequent visits will connect silently.

> **Note:** `tls/generate.sh` is included in the repository.
`tls/cert.pem` and `tls/key.pem` are generated locally and must never be
committed.
