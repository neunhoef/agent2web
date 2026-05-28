/* agent2web — client-side JavaScript
 *
 * Five functional areas:
 *   1. Password field: persist in sessionStorage, inject into every form.
 *   2. Audio capture: MediaRecorder → POST /audio → populate transcript textarea.
 *   3. Auto-scroll: MutationObserver on the agent output div.
 *   4. SSE live output: EventSource on /stream when a run is in progress.
 *   5. New-conversation form toggle: show/hide the inline label form.
 *
 * Total budget: ≤ 300 lines.
 */

'use strict';

// ── 1. Password persistence & injection ─────────────────────────────────────

(function () {
  const SESSION_KEY = 'agent2web_password';
  const pwdField = document.getElementById('password');
  if (!pwdField) return;

  // Restore saved value
  const saved = sessionStorage.getItem(SESSION_KEY);
  if (saved) pwdField.value = saved;

  // Persist on change
  pwdField.addEventListener('input', () => {
    sessionStorage.setItem(SESSION_KEY, pwdField.value);
  });

  // Inject into every form before submission
  document.addEventListener('submit', function (e) {
    const form = e.target;
    if (!form || form.tagName !== 'FORM') return;
    // Remove any previously injected field to avoid duplicates
    const existing = form.querySelector('input[name="password"][data-injected]');
    if (existing) existing.remove();
    // Inject current password
    const hidden = document.createElement('input');
    hidden.type = 'hidden';
    hidden.name = 'password';
    hidden.value = pwdField.value;
    hidden.dataset.injected = '1';
    form.appendChild(hidden);
  });
})();

// ── 2. Audio capture ─────────────────────────────────────────────────────────

(function () {
  const recordBtn = document.getElementById('btn-record');
  const stopBtn = document.getElementById('btn-stop');
  const promptArea = document.getElementById('prompt');
  const recordStatus = document.getElementById('record-status');

  if (!recordBtn || !stopBtn || !promptArea) return;

  let mediaRecorder = null;
  let chunks = [];
  let stream = null;

  function setStatus(msg, isError) {
    if (!recordStatus) return;
    recordStatus.textContent = msg;
    recordStatus.style.color = isError ? 'var(--danger)' : 'var(--text-muted)';
  }

  recordBtn.addEventListener('click', async () => {
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      chunks = [];

      const mimeType = MediaRecorder.isTypeSupported('audio/webm;codecs=opus')
        ? 'audio/webm;codecs=opus'
        : 'audio/webm';

      mediaRecorder = new MediaRecorder(stream, { mimeType });

      // Without a timeslice, ondataavailable fires exactly once when stop()
      // is called, delivering all buffered audio in a single event.  This
      // avoids the race condition where the stop event fires before the last
      // timeslice chunk is delivered in some browsers.
      mediaRecorder.ondataavailable = async (e) => {
        // Release the microphone as soon as data is available.
        stream.getTracks().forEach(t => t.stop());
        stream = null;

        document.body.classList.remove('recording');

        if (!e.data || e.data.size === 0) {
          setStatus('Recording was empty — please try again.', true);
          recordBtn.disabled = false;
          stopBtn.disabled = true;
          return;
        }

        const blob = new Blob([e.data], { type: mimeType });
        const form = new FormData();
        form.append('audio', blob, 'recording.webm');

        // Inject password
        const pwdField = document.getElementById('password');
        if (pwdField) form.append('password', pwdField.value);

        setStatus('Transcribing…');
        recordBtn.disabled = true;
        stopBtn.disabled = true;
        // Disable the prompt area while transcription is in flight.
        promptArea.disabled = true;
        const sendBtn = document.querySelector('#run-form button[type="submit"]');
        if (sendBtn) sendBtn.disabled = true;

        try {
          const res = await fetch('/audio', { method: 'POST', body: form });
          const data = await res.json();
          if (data.transcript) {
            promptArea.value = data.transcript;
            setStatus('Transcript ready — review and edit before sending.');
          } else if (data.error) {
            setStatus('Error: ' + data.error, true);
          }
        } catch (err) {
          setStatus('Upload failed: ' + err.message, true);
        } finally {
          promptArea.disabled = false;
          if (sendBtn) sendBtn.disabled = false;
          recordBtn.disabled = false;
        }
      };

      mediaRecorder.start(); // no timeslice — all data delivered in one ondataavailable
      document.body.classList.add('recording');
      recordBtn.disabled = true;
      stopBtn.disabled = false;
      setStatus('Recording…');
    } catch (err) {
      setStatus('Microphone error: ' + err.message, true);
    }
  });

  stopBtn.addEventListener('click', () => {
    if (mediaRecorder && mediaRecorder.state !== 'inactive') {
      mediaRecorder.stop(); // triggers ondataavailable then stop event
      setStatus('Processing…');
    }
  });

  // Initialise button state
  stopBtn.disabled = true;
})();

// ── 3. Auto-scroll agent output ──────────────────────────────────────────────

(function () {
  const outputBox = document.getElementById('agent-output');
  if (!outputBox) return;

  const observer = new MutationObserver(() => {
    outputBox.scrollTop = outputBox.scrollHeight;
  });

  observer.observe(outputBox, { childList: true, subtree: true, characterData: true });
})();

// ── 4. SSE live output ───────────────────────────────────────────────────────

(function () {
  const outputBox = document.getElementById('agent-output');
  if (!outputBox || !outputBox.dataset.sseRunning) return;

  const source = new EventSource('/stream');

  source.onmessage = function (e) {
    const data = e.data;
    if (data === '__DONE__') {
      source.close();
      // Reload to show final run status and diff.
      window.location.reload();
      return;
    }
    // Remove the "empty" placeholder if present.
    const placeholder = outputBox.querySelector('.output-empty');
    if (placeholder) placeholder.remove();

    // Append a new output line.
    const isStderr = data.startsWith('[stderr] ');
    const span = document.createElement('span');
    span.className = 'output-line' + (isStderr ? ' stderr' : '');
    span.textContent = data;
    outputBox.appendChild(span);
    outputBox.appendChild(document.createTextNode('\n'));
  };

  source.onerror = function () {
    // Connection lost — close and let the page reload handle it.
    source.close();
  };
})();

// ── 5. New-conversation form toggle ─────────────────────────────────────────

(function () {
  const btn = document.getElementById('btn-new-conv');
  const panel = document.getElementById('new-conv-form');
  if (!btn || !panel) return;

  btn.addEventListener('click', function (e) {
    e.stopPropagation();
    panel.hidden = !panel.hidden;
    if (!panel.hidden) {
      const input = panel.querySelector('input[name="label"]');
      if (input) input.focus();
    }
  });

  // Close the panel when clicking outside of it.
  document.addEventListener('click', function (e) {
    if (!panel.hidden && !panel.contains(e.target) && e.target !== btn) {
      panel.hidden = true;
    }
  });
})();
