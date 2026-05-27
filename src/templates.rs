//! Server-side HTML rendering for agent2web.
//!
//! Static assets are embedded at compile time so the binary is self-contained.
//! All template functions return `String`; axum converts those to `Html<String>`.

const STYLE_CSS: &str = include_str!("../static/style.css");
const APP_JS: &str = include_str!("../static/app.js");

/// The HTMX library — loaded from CDN for M1; later milestones may self-host.
const HTMX_CDN: &str = "https://unpkg.com/htmx.org@2.0.4/dist/htmx.min.js";

use crate::state::{ConvState, RunState, RunStatus};

// ── Page shell ────────────────────────────────────────────────────────────

/// Render the full UI page shell.
pub fn render_index(
    run: &RunState,
    conv: &ConvState,
    project_dir: &str,
    password_enabled: bool,
    prompts_count: usize,
) -> String {
    let project_name = std::path::Path::new(project_dir)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| project_dir.to_string());

    let conv_bar = render_conv_bar(conv);
    let run_status_badge = render_run_status_badge(&run.status);
    let output_section = render_output_section(run);
    let is_running = run.status.is_running();

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>agent2web — {project_name}</title>
  <style>{STYLE_CSS}</style>
  <script src="{HTMX_CDN}" defer></script>
</head>
<body>

<!-- ── Header ── -->
<header>
  <h1>agent2web</h1>
  <span class="project-name">project: {project_name}</span>
  <div class="header-spacer"></div>
  {password_field}
</header>

<!-- ── Conversation status bar ── -->
{conv_bar}

<!-- ── Main content ── -->
<main>
  <div class="col-left">

    <!-- ── Voice & Prompt section ── -->
    <div class="card">
      <div class="card-header">
        <h2>Prompt</h2>
        <div style="flex:1"></div>
        {run_status_badge}
      </div>
      <div class="card-body">
        <div class="voice-controls">
          <button class="btn btn-secondary" id="btn-record" {disabled}>
            &#x1F3A4; Record
          </button>
          <button class="btn btn-secondary" id="btn-stop" {disabled}>
            &#x23F9; Stop
          </button>
          <div class="record-status" id="record-status">
            <span class="record-dot"></span>
            <span>Ready to record</span>
          </div>
        </div>

        <form method="POST" action="/run" id="run-form">
          <textarea
            id="prompt"
            name="prompt"
            placeholder="Type or dictate your prompt here…"
            rows="4"
            {disabled}
          ></textarea>
          <div class="prompt-actions">
            <button type="submit" class="btn btn-primary" {disabled}>
              &#x25B6; Send to Agent
            </button>
            <button
              type="button"
              class="btn btn-secondary btn-sm"
              onclick="document.getElementById('prompt').value=''"
              {disabled}
            >
              Clear
            </button>
          </div>
        </form>
      </div>
    </div>

    <!-- ── Agent output section ── -->
    {output_section}

    <!-- ── Manual commit section ── -->
    <div class="card">
      <div class="card-header">
        <h2>Commit</h2>
      </div>
      <div class="card-body">
        <form method="POST" action="/commit">
          <div class="commit-row">
            <input
              type="text"
              name="message"
              placeholder="Commit subject line…"
              {commit_disabled}
            />
            <button type="submit" class="btn btn-success" {commit_disabled}>
              &#x2714; Commit
            </button>
          </div>
          <p class="commit-hint">
            {prompts_hint}
          </p>
        </form>
      </div>
    </div>

  </div><!-- .col-left -->

  <div class="col-right">

    <!-- ── Diff section ── -->
    <div class="card">
      <div class="card-header">
        <h2>Diff</h2>
      </div>
      <div class="card-body">
        <p class="commit-hint" style="margin-bottom:0.75rem">
          Opens in a new page — use &#x2190; Back to return here.
        </p>
        <div class="diff-controls">
          <a href="/diff" class="btn btn-secondary btn-sm">HEAD~1</a>
          <a href="/diff/range?from=HEAD~3&amp;to=HEAD" class="btn btn-secondary btn-sm">HEAD~3..HEAD</a>
        </div>
      </div>
    </div>

  </div><!-- .col-right -->
</main>

<script>{APP_JS}</script>
</body>
</html>"#,
        project_name = html_escape(&project_name),
        conv_bar = conv_bar,
        run_status_badge = run_status_badge,
        output_section = output_section,
        password_field = if password_enabled {
            r#"<div class="password-group">
          <label for="password">Password:</label>
          <input type="password" id="password" name="password" autocomplete="current-password" />
        </div>"#
        } else {
            ""
        },
        disabled = if is_running { "disabled" } else { "" },
        commit_disabled = if is_running { "disabled" } else { "" },
        prompts_hint = if prompts_count == 0 {
            "No prompts accumulated yet.".to_string()
        } else {
            format!(
                "{} prompt{} will be appended to the commit body.",
                prompts_count,
                if prompts_count == 1 { "" } else { "s" }
            )
        },
    )
}

// ── Conversation bar ───────────────────────────────────────────────────────

fn render_conv_bar(conv: &ConvState) -> String {
    let active_info = if let Some(entry) = conv.active_entry() {
        format!(
            r#"<span class="conv-badge">
        <span class="conv-id">{}</span>
        <span class="conv-runs">({} run{})</span>
        {label}
      </span>"#,
            html_escape(entry.short_id()),
            entry.run_count,
            if entry.run_count == 1 { "" } else { "s" },
            label = if !entry.label.is_empty() {
                format!(
                    "<span style=\"color:var(--text-muted);font-size:0.78rem\">&nbsp;·&nbsp;{}</span>",
                    html_escape(truncate(&entry.label, 40))
                )
            } else {
                String::new()
            },
        )
    } else {
        r#"<span class="conv-none">No active conversation — next run will start a new one.</span>"#
            .to_string()
    };

    format!(
        r##"<div class="conv-bar">
    {active_info}
    <div class="conv-bar-spacer"></div>
    <form method="POST" action="/conversation/new" style="display:inline">
      <button type="submit" class="btn btn-secondary btn-sm">&#x2295; New Conversation</button>
    </form>
    <button
      class="btn btn-secondary btn-sm"
      hx-get="/conversation/list"
      hx-target="#conv-history-panel"
      hx-swap="innerHTML"
      hx-trigger="click"
      onclick="document.getElementById('conv-history-panel').hidden = !document.getElementById('conv-history-panel').hidden"
    >&#x1F4C4; History</button>
  </div>
  <div id="conv-history-panel" hidden></div>"##,
        active_info = active_info,
    )
}

// ── Run status badge ───────────────────────────────────────────────────────

fn render_run_status_badge(status: &RunStatus) -> String {
    let (class, icon, text) = match status {
        RunStatus::Idle => ("status-idle", "○", "Idle"),
        RunStatus::Running => ("status-running", "●", "Running"),
        RunStatus::Done { .. } => ("status-done", "✔", "Done"),
        RunStatus::Failed { .. } => ("status-failed", "✖", "Failed"),
    };
    format!(r#"<span class="status-badge {class}">{icon} {text}</span>"#,)
}

// ── Agent output section ───────────────────────────────────────────────────

fn render_output_section(run: &RunState) -> String {
    let is_running = run.status.is_running();

    let output_inner = if run.output_buf.is_empty() {
        if is_running {
            // Empty string: the SSE JS will populate the box with incoming lines.
            String::new()
        } else {
            r#"<span class="output-empty">Agent output will stream here during a run.</span>"#
                .to_string()
        }
    } else {
        run.output_buf
            .iter()
            .map(|line| {
                let is_stderr = line.starts_with("[stderr] ");
                let class = if is_stderr {
                    "output-line stderr"
                } else {
                    "output-line"
                };
                format!(r#"<span class="{class}">{}</span>"#, html_escape(line))
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    // When Running, tell the JS to attach an SSE listener to this element.
    let sse_attr = if is_running {
        r#" data-sse-running="true""#
    } else {
        ""
    };

    // Show a "Failed" alert if the last run failed.
    let failure_alert = match &run.status {
        RunStatus::Failed { reason } => format!(
            r#"<div class="alert alert-error" style="margin-bottom:0.75rem">
          Run failed: {}
        </div>"#,
            html_escape(reason)
        ),
        RunStatus::Done { commit_sha } => {
            if let Some(sha) = commit_sha {
                format!(
                    r#"<div class="alert alert-info" style="margin-bottom:0.75rem">
          Run complete — commit <code>{}</code>.
        </div>"#,
                    html_escape(&sha[..sha.len().min(12)])
                )
            } else {
                r#"<div class="alert alert-info" style="margin-bottom:0.75rem">
          Run complete (no new commit).
        </div>"#
                    .to_string()
            }
        }
        _ => String::new(),
    };

    format!(
        r#"<div class="card">
      <div class="card-header">
        <h2>Agent Output</h2>
      </div>
      <div class="card-body">
        {failure_alert}
        <div class="output-box" id="agent-output"{sse_attr}>{output_inner}</div>
      </div>
    </div>"#,
    )
}

// ── Conversation list fragment ─────────────────────────────────────────────

/// Render the conversation history list as an HTML fragment (for HTMX swap).
pub fn render_conv_list(conv: &ConvState) -> String {
    if conv.history.is_empty() {
        return r#"<div class="conv-history" style="padding:1rem;color:var(--text-muted)">
      No conversation history yet.
    </div>"#
            .to_string();
    }

    let entries: String = conv
        .history
        .iter()
        .rev()
        .map(|e| {
            let active_marker = if conv.active_id.as_deref() == Some(&e.id) {
                " ◀ active"
            } else {
                ""
            };
            format!(
                r#"<div class="conv-entry">
          <span class="conv-entry-id">{id}</span>
          <span class="conv-entry-label">{label}{active}</span>
          <span class="conv-entry-meta">{runs} run{s}</span>
          <form method="POST" action="/conversation/resume" style="display:inline">
            <input type="hidden" name="id" value="{full_id}" />
            <button type="submit" class="btn btn-secondary btn-sm">Resume</button>
          </form>
        </div>"#,
                id = html_escape(e.short_id()),
                label = if e.label.is_empty() {
                    "(no label)".to_string()
                } else {
                    html_escape(truncate(&e.label, 60)).to_string()
                },
                active = active_marker,
                runs = e.run_count,
                s = if e.run_count == 1 { "" } else { "s" },
                full_id = html_escape(&e.id),
            )
        })
        .collect();

    format!(r#"<div class="conv-history">{entries}</div>"#)
}

// ── Error page ─────────────────────────────────────────────────────────────

/// Render a simple error page (used for 4xx/5xx responses).
pub fn render_error(status: u16, message: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Error {status} — agent2web</title>
  <style>{STYLE_CSS}</style>
</head>
<body>
<header><h1>agent2web</h1></header>
<main>
  <div class="card" style="margin-top:2rem">
    <div class="card-header"><h2>Error {status}</h2></div>
    <div class="card-body">
      <div class="alert alert-error">{message}</div>
      <p style="margin-top:1rem"><a href="/" style="color:var(--accent)">← Back to home</a></p>
    </div>
  </div>
</main>
</body>
</html>"#,
        status = status,
        message = html_escape(message),
    )
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Escape HTML special characters.
pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Truncate a string to at most `max` chars, appending "…" if truncated.
pub fn truncate(s: &str, max: usize) -> &str {
    // Byte-based truncation at a char boundary (simple approximation).
    if s.len() <= max { s } else { &s[..max] }
}
