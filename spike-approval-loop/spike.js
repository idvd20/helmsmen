#!/usr/bin/env node
'use strict';

// PROTOTYPE (spike-approval-loop) — throwaway driver, delete when NOTES.md is filled.
//
// One process, two jobs:
//   1. Dummy control-plane server: receives hook POSTs on :4519 (/event/<type>),
//      appends them to events.jsonl, folds them through correlate.js.
//   2. A TUI that shows derived inbox cards, raw payloads, and a live snapshot of
//      the claude tmux pane, with keys to launch/prompt/allow/deny.
//
// The keystroke injection itself lives in answer-prompt.sh (the seam) — this file
// never calls tmux send-keys for prompt-answering directly.

const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const readline = require('node:readline');
const { execFile, execFileSync } = require('node:child_process');
const { promisify } = require('node:util');
const { emptyState, applyEvent } = require('./correlate.js');

const pExecFile = promisify(execFile);

const PORT = Number(process.env.HELMSMEN_SPIKE_PORT || 4519);
const SESSION = 'helmsmen-spike';
const ROOT = __dirname;
const WORKDIR = path.join(ROOT, 'workdir');
const SEAM = path.join(ROOT, 'answer-prompt.sh');
const EVENTS_LOG = path.join(ROOT, 'events.jsonl');
const CANNED_PROMPT =
  'Run exactly one bash command: git log --oneline -3. Report the output, then stop.';

// ---------------------------------------------------------------------------
// mutable spike state (in-memory only — persistence is not what we're testing)
let events = [];
let corr = emptyState();
let seq = 0;
let paneLines = ['(no tmux session — press [c] to launch claude)'];
let tmuxAlive = false;
let showPayload = false;
let statusMsg = 'ready';
let lineInputActive = false;

const isTTY = process.stdin.isTTY && process.stdout.isTTY;

// ---------------------------------------------------------------------------
// control-plane dummy server
const server = http.createServer((req, res) => {
  if (req.method === 'GET' && req.url === '/events') {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ events, state: corr }, null, 2));
    return;
  }
  const m = req.method === 'POST' && /^\/event\/([a-z]+)$/.exec(req.url || '');
  if (!m) {
    res.writeHead(404);
    res.end();
    return;
  }
  let body = '';
  req.on('data', (chunk) => (body += chunk));
  req.on('end', () => {
    let payload;
    try {
      payload = JSON.parse(body);
    } catch {
      payload = { unparseable: body.slice(0, 500) };
    }
    const event = { seq: ++seq, receivedAt: new Date().toISOString(), type: m[1], payload };
    events.push(event);
    fs.appendFile(EVENTS_LOG, JSON.stringify(event) + '\n', () => {});
    corr = applyEvent(corr, event);
    if (isTTY) render();
    else console.log(`[event] ${event.type} ${payload.tool_name || payload.message || ''}`);
    res.writeHead(200);
    res.end('ok');
  });
});

// ---------------------------------------------------------------------------
// tmux plumbing (launch/observe only — answering goes through the seam)
async function refreshTmux() {
  try {
    await pExecFile('tmux', ['has-session', '-t', SESSION]);
    tmuxAlive = true;
    const { stdout } = await pExecFile('tmux', ['capture-pane', '-p', '-t', SESSION]);
    const lines = stdout.replace(/\s+$/, '').split('\n');
    paneLines = lines.slice(-16).map((l) => l.slice(0, 118));
  } catch {
    tmuxAlive = false;
    paneLines = ['(no tmux session — press [c] to launch claude)'];
  }
}

function ensureWorkdir() {
  fs.mkdirSync(path.join(WORKDIR, '.claude', 'hooks'), { recursive: true });
  fs.copyFileSync(
    path.join(ROOT, 'templates', 'settings.json'),
    path.join(WORKDIR, '.claude', 'settings.json')
  );
  const hook = path.join(WORKDIR, '.claude', 'hooks', 'post-event.sh');
  fs.copyFileSync(path.join(ROOT, 'templates', 'hooks', 'post-event.sh'), hook);
  fs.chmodSync(hook, 0o755);
  if (!fs.existsSync(path.join(WORKDIR, '.git'))) {
    fs.writeFileSync(
      path.join(WORKDIR, 'README.md'),
      'PROTOTYPE sandbox — wipe me. Claude runs here so its project root (and hook\n' +
        'config) is unambiguously this directory, not the helmsmen repo.\n'
    );
    // Own git repo → CLAUDE_PROJECT_DIR resolves here; no remote → pushes fail harmlessly.
    execFileSync('git', ['-C', WORKDIR, 'init'], { stdio: 'ignore' });
    execFileSync('git', ['-C', WORKDIR, 'add', '-A'], { stdio: 'ignore' });
    try {
      execFileSync('git', ['-C', WORKDIR, 'commit', '-m', 'sandbox seed'], { stdio: 'ignore' });
    } catch {
      // no git identity — fine, `git status` still works for the canned prompt
    }
  }
}

async function launchClaude() {
  if (tmuxAlive) {
    statusMsg = `session ${SESSION} already running — [x] to kill it first`;
    return;
  }
  ensureWorkdir();
  await pExecFile('tmux', [
    'new-session', '-d', '-s', SESSION, '-x', '220', '-y', '50', '-c', WORKDIR,
    '-e', `HELMSMEN_SPIKE_PORT=${PORT}`,
    'claude',
  ]);
  statusMsg = `claude launched in tmux:${SESSION} — first run may show a trust prompt ` +
    `(answer via [k] raw keys or \`tmux attach -t ${SESSION}\`)`;
}

async function killSession() {
  try {
    await pExecFile('tmux', ['kill-session', '-t', SESSION]);
    statusMsg = 'tmux session killed';
  } catch {
    statusMsg = 'no session to kill';
  }
}

function sendPromptToClaude(text) {
  return pExecFile('tmux', ['send-keys', '-t', SESSION, '-l', text]).then(
    () =>
      new Promise((resolve) =>
        setTimeout(() => pExecFile('tmux', ['send-keys', '-t', SESSION, 'Enter']).then(resolve), 150)
      )
  );
}

function answerViaSeam(action, message) {
  const args = [SESSION, action];
  if (message) args.push(message);
  return new Promise((resolve) => {
    execFile(SEAM, args, (err) => {
      statusMsg = err ? `seam failed: ${err.message}` : `seam: ${action} sent`;
      resolve();
    });
  });
}

// ---------------------------------------------------------------------------
// rendering
const B = (s) => `\x1b[1m${s}\x1b[0m`;
const D = (s) => `\x1b[2m${s}\x1b[0m`;
const Y = (s) => `\x1b[33m${s}\x1b[0m`;

const STATUS_PAINT = {
  pending: (s) => Y(s),
  surfaced: (s) => B(Y(s)),
  allowed: (s) => `\x1b[32m${s}\x1b[0m`,
  'closed-no-run': (s) => D(s),
};

function cardLine(c) {
  const input =
    typeof c.toolInput.command === 'string'
      ? c.toolInput.command
      : JSON.stringify(c.toolInput);
  const paint = STATUS_PAINT[c.status] || ((s) => s);
  return (
    `  #${String(c.seq).padEnd(3)} ${paint(c.status.padEnd(13))} ` +
    `${c.toolName.padEnd(6)} ${input.slice(0, 60).padEnd(62)} ` +
    D(`notif${c.notification ? '✓' : '·'} tool_use_id${c.toolUseId ? '✓' : '·'} ${c.sessionId.slice(0, 8)}`)
  );
}

function render() {
  if (!isTTY || lineInputActive) return;
  const f = [];
  f.push(B('HELMSMEN SPIKE — approval loop (ask + send-keys)') + D(`   server :${PORT} | tmux ${tmuxAlive ? 'alive' : 'down'} | events ${events.length}`));
  f.push(D('question: does PreToolUse `ask` + tmux send-keys close the loop? criteria in README.md, verdict in NOTES.md'));
  f.push('');

  f.push(B('Inbox cards') + D(' (derived by correlate.js — pending → surfaced → allowed | closed-no-run)'));
  const cards = corr.cards.slice(-6);
  f.push(...(cards.length ? cards.map(cardLine) : [D('  (none yet — [c] launch, then [p] send the test prompt)')]));
  if (corr.warnings.length) {
    f.push(Y(B(`  ⚠ correlation warnings (criterion 4): ${corr.warnings.length}`)));
    f.push(...corr.warnings.slice(-3).map((w) => Y(`    ${w.slice(0, 116)}`)));
  }
  f.push('');

  if (showPayload && events.length) {
    const last = events[events.length - 1];
    f.push(B(`Last event payload`) + D(` — #${last.seq} ${last.type} (full log: events.jsonl)`));
    f.push(...JSON.stringify(last, null, 1).split('\n').slice(0, 18).map((l) => D('  ' + l.slice(0, 116))));
    f.push('');
  }

  f.push(B(`claude pane`) + D(` — tmux:${SESSION}, snapshot every 2s (attach for the real thing)`));
  f.push(D('  ┌' + '─'.repeat(110)));
  f.push(...paneLines.map((l) => D('  │ ') + l.slice(0, 110)));
  f.push(D('  └' + '─'.repeat(110)));
  f.push('');

  f.push(D('status: ') + statusMsg.slice(0, 110));
  f.push(
    `${B('[c]')}${D(' launch claude ')}${B('[p]')}${D(' send prompt ')}${B('[a]')}${D(' allow ')}` +
      `${B('[d]')}${D(' deny+reason ')}${B('[k]')}${D(' raw keys ')}${B('[v]')}${D(' payload ')}` +
      `${B('[x]')}${D(' kill tmux ')}${B('[r]')}${D(' refresh ')}${B('[q]')}${D(' quit')}`
  );
  process.stdout.write('\x1b[2J\x1b[H' + f.join('\n') + '\n');
}

// ---------------------------------------------------------------------------
// input
function askLine(promptText, preset) {
  lineInputActive = true;
  process.stdin.setRawMode(false);
  return new Promise((resolve) => {
    const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
    rl.question(`\n${promptText}${preset ? D(` [${preset}]`) : ''}: `, (answer) => {
      rl.close();
      process.stdin.setRawMode(true);
      process.stdin.resume();
      lineInputActive = false;
      resolve(answer.trim() || preset || '');
    });
  });
}

async function handleKey(key) {
  if (lineInputActive) return;
  switch (key) {
    case 'c':
      await launchClaude().catch((e) => (statusMsg = `launch failed: ${e.message}`));
      break;
    case 'p': {
      if (!tmuxAlive) { statusMsg = 'no session — [c] first'; break; }
      const text = await askLine('prompt for claude', CANNED_PROMPT);
      await sendPromptToClaude(text).catch((e) => (statusMsg = `send failed: ${e.message}`));
      statusMsg = 'prompt sent — watch for the card, then the permission dialog in the pane';
      break;
    }
    case 'a':
      await answerViaSeam('allow');
      break;
    case 'd': {
      const reason = await askLine('deny instruction', 'Do not run that. Explain what you would have done instead.');
      await answerViaSeam('deny', reason);
      break;
    }
    case 'k': {
      const keys = await askLine('raw tmux send-keys args (space-separated, e.g. `Enter` or `3` or `Down Enter`)');
      if (keys) {
        await new Promise((res) =>
          execFile(SEAM, [SESSION, 'raw', ...keys.split(/\s+/)], (err) => {
            statusMsg = err ? `raw send failed: ${err.message}` : `raw keys sent: ${keys}`;
            res();
          })
        );
      }
      break;
    }
    case 'v':
      showPayload = !showPayload;
      break;
    case 'x':
      await killSession();
      break;
    case 'r':
      break; // fallthrough to refresh+render below
    case 'q':
    case '\x03': // ctrl-c
      shutdown();
      return;
  }
  await refreshTmux();
  render();
}

function shutdown() {
  if (isTTY) process.stdin.setRawMode(false);
  process.stdout.write(
    '\x1b[2J\x1b[H' +
      `spike server stopped. tmux session ${tmuxAlive ? `still running — \`tmux attach -t ${SESSION}\` or rerun and [x]` : 'not running'}.\n` +
      'evidence: spike-approval-loop/events.jsonl — verdict goes in NOTES.md\n'
  );
  process.exit(0);
}

// ---------------------------------------------------------------------------
// main
if (process.argv.includes('--setup-workdir')) {
  ensureWorkdir();
  console.log(`workdir ready: ${WORKDIR}`);
  process.exit(0);
}

server.listen(PORT, '127.0.0.1', async () => {
  await refreshTmux();
  if (!isTTY) {
    console.log(`spike server on :${PORT} (no TTY — server-only mode, events echo below)`);
    return;
  }
  process.stdin.setRawMode(true);
  process.stdin.resume();
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', (k) => void handleKey(k));
  setInterval(async () => {
    if (lineInputActive) return;
    await refreshTmux();
    render();
  }, 2000);
  render();
});
server.on('error', (e) => {
  console.error(`server failed on :${PORT} — ${e.message} (another spike running?)`);
  process.exit(1);
});
