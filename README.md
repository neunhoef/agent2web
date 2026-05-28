# agent2web

A self-hosted Rust web server that acts as a mobile-friendly control panel for a
local [ForgeCode](https://forge.codes) AI coding agent.  It closes the loop
between a developer on the go and a running ForgeCode instance on a home server:

1. Dictate a task via voice on a mobile browser.
2. Review and edit the transcribed text before dispatching.
3. The server runs ForgeCode non-interactively, streaming live output back to
   the browser.
4. Review the working-tree diff once the agent finishes.
5. Select exactly which files to commit, write a subject line, and commit.
   The accumulated agent prompts are appended to the commit body automatically.
6. Repeat — each subsequent run continues the same ForgeCode conversation by
   default, preserving context across turns.

The agent **never commits automatically**.  All commits are initiated by you
from the commit page.

---

## Features

- **Voice input** — record audio in the browser, upload to the server, and get a
  high-quality transcript back in under a second (using a local `whisper-server`
  or a cloud API).
- **Editable transcript** — review and correct the transcript before sending it
  to the agent.
- **Live streaming** — agent stdout/stderr streams to the browser in real time
  via Server-Sent Events.
- **Syntax-highlighted diffs** — working-tree diff (`git diff HEAD`), single-
  commit diff, and range diff, all rendered via `diff2html-cli`.
- **Selective commit** — choose which modified files to stage; the commit body
  is automatically populated with all prompts issued since the last commit,
  word-wrapped with `par`.
- **Conversation continuity** — ForgeCode conversation state persists across
  runs.  Explicit UI controls to start a new conversation or switch to a past
  one.
- **Optional TLS** — built-in TLS listener (rustls) for direct HTTPS without a
  reverse proxy.
- **Optional password auth** — single shared password protecting all mutating
  endpoints.
- **Single binary** — no runtime Rust dependencies; all assets (CSS, JS) are
  embedded at compile time.

---

## Prerequisites

### Required

| Dependency | Minimum version | Notes |
|------------|----------------|-------|
| **Rust** | 1.85 (edition 2024) | Install via [rustup](https://rustup.rs) |
| **forge** | any | ForgeCode binary on `$PATH` |
| **git** | any | Must be on `$PATH` in the working directory |
| **ffmpeg** | any | Audio format conversion (`webm/opus` → 16 kHz WAV) |
| **node** + **diff2html-cli** | node ≥ 18 | Diff rendering — see below |

### Optional but recommended

| Dependency | Notes |
|------------|-------|
| **par** | Paragraph formatter used to word-wrap the commit body to 72 columns. If absent the body is included without reflowing. Install: `apt install par` or `brew install par`. |
| **whisper-server** | Fully local, GPU-accelerated STT — recommended for best quality and zero latency. See [Local whisper-server setup](#local-whisper-server-setup). |

### Installing diff2html-cli

```sh
npm install -g diff2html-cli
```

Verify: `diff2html --version`

### Installing ffmpeg

```sh
# Debian/Ubuntu
sudo apt install ffmpeg

# macOS
brew install ffmpeg
```

---

## Building

```sh
git clone <this-repo>
cd agent2web
cargo build --release
```

The compiled binary is at `target/release/agent2web`.  It is fully
self-contained — CSS and JavaScript are embedded at compile time.

---

## Quick start

```sh
# 1. Copy the example config and edit it for your environment.
cp agent2web.toml.example agent2web.toml
$EDITOR agent2web.toml   # at minimum set project_dir

# 2. Run the server.
./target/release/agent2web --config agent2web.toml

# 3. Open http://localhost:8080 in a browser.
```

---

## Configuration

Configuration is read from a TOML file (default: `./agent2web.toml`).
Pass a different path with `--config <path>`.

```toml
[server]
# Socket address to bind.
bind         = "0.0.0.0:8080"

# Absolute path to the git repository ForgeCode operates on.
# This is the directory where forge and git commands are run.
project_dir  = "/home/user/myproject"

# Maximum seconds a forge run may run before being killed.
# Default: 600 (10 minutes).
run_timeout  = 600

# Shared password for all mutating endpoints (POST /run, POST /audio,
# POST /commit, POST /conversation/new, POST /conversation/resume).
# Leave empty (the default) to disable authentication entirely.
# Override at runtime with the AGENT2WEB_PASSWORD environment variable.
password     = ""


# ── Optional TLS ─────────────────────────────────────────────────────────────
# Enable when not sitting behind a TLS-terminating reverse proxy (e.g. nginx
# or Caddy).  Run tls/generate.sh once to create a self-signed certificate.
[server.tls]
enabled = false
cert    = "tls/cert.pem"   # path relative to the config file, or absolute
key     = "tls/key.pem"


# ── ForgeCode binary ──────────────────────────────────────────────────────────
[forge]
# Name or absolute path of the forge binary.
binary = "forge"


# ── Speech-to-text ────────────────────────────────────────────────────────────
[stt]
# Which provider to use: "whisper_server" (default), "openai", or "deepgram".
provider = "whisper_server"

# API key for cloud providers (openai / deepgram).
# Override with the AGENT2WEB_STT_API_KEY environment variable.
api_key  = ""

# Settings for the local whisper-server provider.
[stt.whisper_server]
url = "http://127.0.0.1:8090/v1/audio/transcriptions"

# Settings for the OpenAI provider.
[stt.openai]
model = "whisper-1"

# (Deepgram has no additional settings beyond api_key.)


# ── Diff rendering ────────────────────────────────────────────────────────────
[diff]
# Number of recent commit SHAs to keep in memory (for future diff history UI).
max_history   = 20

# Lines of context shown around each change in the diff output.
context_lines = 5
```

### Environment variable overrides

| Variable | Overrides |
|----------|-----------|
| `AGENT2WEB_PASSWORD` | `server.password` |
| `AGENT2WEB_STT_API_KEY` | `stt.api_key` |

### Logging

Log verbosity is controlled by the `RUST_LOG` environment variable:

```sh
RUST_LOG=agent2web=debug ./agent2web
RUST_LOG=agent2web=info,tower_http=debug ./agent2web
```

Default level: `info`.

---

## Speech-to-text providers

### Local whisper-server (recommended)

`whisper.cpp` ships a built-in HTTP server that loads the model once into GPU
VRAM and serves subsequent requests with near-zero overhead.  Warm inference on
a mid-range NVIDIA GPU takes 150–500 ms for a 5–30 second clip.

**Build with CUDA support:**

```sh
git clone https://github.com/ggml-org/whisper.cpp
cd whisper.cpp
cmake -B build -DGGML_CUDA=1
cmake --build build -j --config Release

# Download the large-v3-turbo model (~1.6 GB FP16, ~800 MB Q5_0):
bash ./models/download-ggml-model.sh large-v3-turbo
# Or the smaller quantised variant:
# bash ./models/download-ggml-model.sh large-v3-turbo-q5_0
```

**Run as a persistent service:**

```sh
./build/bin/whisper-server \
  --model models/ggml-large-v3-turbo.bin \
  --host 127.0.0.1 \
  --port 8090 \
  --inference-path /v1/audio/transcriptions \
  --threads 4 \
  --processors 1
```

**Or install as a systemd service** so it starts automatically and the model
stays warm in VRAM:

```ini
# /etc/systemd/system/whisper-server.service
[Unit]
Description=whisper-server STT
After=network.target

[Service]
ExecStart=/path/to/whisper-server \
  --model /path/to/ggml-large-v3-turbo.bin \
  --host 127.0.0.1 --port 8090 \
  --inference-path /v1/audio/transcriptions
Restart=always
Environment=CUDA_VISIBLE_DEVICES=0

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl enable --now whisper-server
```

**Configuration:**

```toml
[stt]
provider = "whisper_server"

[stt.whisper_server]
url = "http://127.0.0.1:8090/v1/audio/transcriptions"
```

### OpenAI Whisper API

```toml
[stt]
provider = "openai"
api_key  = "sk-..."   # or set AGENT2WEB_STT_API_KEY

[stt.openai]
model = "whisper-1"
```

Cost: ~$0.006/minute — roughly $0.003 per 30-second dictation clip.

### Deepgram Nova-3

```toml
[stt]
provider = "deepgram"
api_key  = "..."   # or set AGENT2WEB_STT_API_KEY
```

Cost: ~$0.004/minute.  Strong on technical vocabulary; low latency.

---

## TLS setup

When running directly on the network without a reverse proxy, enable the
built-in TLS listener.  A helper script generates a 10-year self-signed
RSA-4096 certificate:

```sh
bash tls/generate.sh
```

This creates `tls/cert.pem` and `tls/key.pem` (both git-ignored).

Then enable TLS in the config:

```toml
[server.tls]
enabled = true
cert    = "tls/cert.pem"
key     = "tls/key.pem"
```

The first time you visit the server in a browser it will show a certificate
warning.  Accept it once (or install the certificate as a trusted CA on your
devices) and subsequent visits will connect silently.

When running behind nginx, Caddy, or another TLS-terminating proxy, leave TLS
disabled and let the proxy handle it.

---

## Password authentication

Set `server.password` (or `AGENT2WEB_PASSWORD`) to enable a shared password.
The UI stores it in `sessionStorage` and injects it automatically into every
form and audio upload, so you only type it once per browser session.

When the password is empty (the default), no authentication is performed.

All read-only endpoints (`GET /`, `GET /diff*`, `GET /commit`,
`GET /conversation/list`, `GET /stream`, `GET /health`) are never
password-protected.

---

## HTTP API

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/` | Main UI page |
| `GET` | `/health` | `200 OK` plain-text health check |
| `POST` | `/audio` | Upload audio blob → JSON transcript |
| `POST` | `/run` | Submit prompt → start agent run → `303 /` |
| `GET` | `/stream` | Server-Sent Events: live agent output |
| `GET` | `/diff` | Working-tree diff (`git diff HEAD`) via diff2html |
| `GET` | `/diff/commit?ref=REF` | Single-commit diff (default `ref=HEAD`) |
| `GET` | `/diff/range?from=SHA&to=SHA` | Range diff |
| `GET` | `/commit` | Commit page: file selector + subject input |
| `POST` | `/commit` | Execute commit with selected files |
| `POST` | `/conversation/new` | Start a new ForgeCode conversation |
| `GET` | `/conversation/list` | HTML fragment: conversation history |
| `POST` | `/conversation/resume` | Resume a past conversation by ID |

### `POST /audio`

Multipart form fields:

| Field | Required | Description |
|-------|----------|-------------|
| `audio` | yes | Recorded audio blob (`audio/webm;codecs=opus` or any format ffmpeg can read). Max 25 MB. |
| `password` | if auth enabled | Shared password. |

Responses:

```json
{ "transcript": "the recognised text" }
{ "error": "human-readable reason" }
```

### `POST /run`

Form fields: `prompt` (required), `password` (if auth enabled).

Returns `303 See Other → /` on success, or an HTML error page on failure.

### `POST /commit`

Form fields: `message` (commit subject, required), `files[]` (one or more file
paths), `include_prompts` (`on` to append prompts to commit body), `password`
(if auth enabled).

File paths are validated against the current `git status --porcelain` output —
only known modified files can be staged.

---

## Usage workflow

```
┌─────────────────────────────────────────────────┐
│  1. Open http(s)://your-server:8080              │
│  2. Click Record, dictate your task, click Stop  │
│  3. Review / edit the transcript textarea        │
│  4. Click Send to Agent                          │
│  5. Watch live output stream                     │
│  6. Click View Working-Tree Diff to review       │
│  7. Click Review & Commit                        │
│     a. Deselect any files you don't want staged  │
│     b. Enter a commit subject line               │
│     c. Click Commit Selected Files               │
│  8. Repeat from step 2 (conversation continues)  │
└─────────────────────────────────────────────────┘
```

To start a fresh ForgeCode conversation, click **New Conversation** in the
conversation status bar.  Past conversations are listed in the **History**
panel and can be resumed at any time.

---

## Project layout

```
agent2web/
├── src/
│   ├── main.rs            entry point, config loading, server startup
│   ├── config.rs          Config struct and TOML deserialisation
│   ├── router.rs          axum Router with all route registrations
│   ├── state.rs           AppState, RunState, ConvState
│   ├── agent.rs           forge subprocess management
│   ├── diff.rs            git diff invocation and diff2html bridge
│   ├── templates.rs       server-side HTML rendering
│   ├── handlers/
│   │   ├── audio.rs       POST /audio — multipart upload + ffmpeg + STT
│   │   ├── commit.rs      GET/POST /commit
│   │   ├── conversation.rs  /conversation/*
│   │   ├── diff.rs        GET /diff*
│   │   ├── health.rs      GET /health
│   │   ├── index.rs       GET /
│   │   ├── run.rs         POST /run
│   │   └── stream.rs      GET /stream (SSE)
│   └── stt/
│       ├── mod.rs         SttProvider trait + factory
│       ├── whisper_api.rs OpenAI-compatible provider (whisper-server + OpenAI)
│       ├── deepgram.rs    Deepgram Nova-3 provider
│       └── whisper_cpp.rs M8 stub (not yet implemented)
├── static/
│   ├── style.css          mobile-first CSS (embedded at compile time)
│   └── app.js             hand-written JS: audio capture, SSE, password (embedded)
├── tls/
│   └── generate.sh        one-shot self-signed certificate generator
└── agent2web.toml.example annotated example configuration
```

---

## Security notes

- Subprocesses (`forge`, `git`, `ffmpeg`, `diff2html`) are spawned via
  `tokio::process::Command` with explicit argument lists — prompt text is
  never interpolated into a shell string.
- Conversation IDs are validated against the in-memory history before being
  passed to `forge`.
- File paths submitted to `POST /commit` are validated against the live
  `git status --porcelain` output — arbitrary paths are rejected.
- Audio uploads are limited to 25 MB.
- When deployed over a network, use TLS (built-in or via a reverse proxy) and
  set a password.  The server is designed for single-user personal use.

---

## License

MIT
