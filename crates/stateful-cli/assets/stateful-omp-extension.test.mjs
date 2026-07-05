import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { pathToFileURL } from 'node:url';

function tempDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'stateful-omp-extension-'));
}

function writeFakeStateful(dir) {
  const bin = path.join(dir, 'stateful-fake.js');
  fs.writeFileSync(
    bin,
    String.raw`#!/usr/bin/env node
const fs = require('node:fs');
const logPath = process.env.STATEFUL_FAKE_LOG;
const args = process.argv.slice(2);
const stdin = fs.readFileSync(0, 'utf8');
const existing = logPath && fs.existsSync(logPath)
  ? fs.readFileSync(logPath, 'utf8').trim().split('\n').filter(Boolean).map((line) => JSON.parse(line))
  : [];
const record = { args, stdin: stdin ? JSON.parse(stdin) : null };
if (logPath) fs.appendFileSync(logPath, JSON.stringify(record) + '\n');
function json(value) { process.stdout.write(JSON.stringify(value)); }
if (args[0] === 'hook' && args[1] === 'omp' && args[2] === 'pre-tool-use') {
  if (record.stdin && record.stdin.tool_name === 'write' && record.stdin.yolo === true) {
    json({ decision: 'allow' });
  } else {
    json({
      decision: 'block',
      reason: 'active claim conflict; wait_id wait-queued',
      wait: {
        wait_id: 'wait-queued',
        reservation_id: 'reservation-queued',
        status: 'queued',
        queue_position: 1
      }
    });
  }
  process.exit(0);
}
if (args[0] === 'hook' && args[1] === 'omp' && args[2] === 'post-tool-use') process.exit(0);
if ((args[0] === 'resume' && args[1] === 'next') || (args[0] === 'notifications' && args[1] === 'poll')) {
  json({
    resume_available: true,
    reservation: {
      wait_id: 'wait-queued',
      reservation_id: 'reservation-queued',
      status: 'reserved',
      relative_path: 'src/app.txt',
      action: 'write_file',
      purpose: 'finish queued write'
    }
  });
  process.exit(0);
}
if (args[0] === 'reservation' && args[1] === 'claim') {
  const waited = existing.some((entry) =>
    (entry.args[0] === 'resume' && entry.args[1] === 'next')
      || (entry.args[0] === 'notifications' && entry.args[1] === 'poll')
  );
  if (!waited) {
    process.stderr.write('reservation not found');
    process.exit(1);
  }
  json({ reservation: { wait_id: 'wait-queued', reservation_id: 'reservation-queued', status: 'claimed' } });
  process.exit(0);
}
process.stderr.write('unexpected fake stateful call: ' + args.join(' '));
process.exit(1);
`,
    { mode: 0o755 },
  );
  return bin;
}

async function loadExtension(t, fakeStateful, logPath) {
  const dir = tempDir();
  const source = fs.readFileSync(path.resolve('crates/stateful-cli/assets/stateful-omp-extension.js'), 'utf8');
  const modulePath = path.join(dir, 'stateful-omp-extension.mjs');
  fs.writeFileSync(modulePath, source.replace('__STATEFUL_BINARY_JSON__', JSON.stringify(fakeStateful)));

  const previousLog = process.env.STATEFUL_FAKE_LOG;
  process.env.STATEFUL_FAKE_LOG = logPath;
  t.after(() => {
    if (previousLog === undefined) delete process.env.STATEFUL_FAKE_LOG;
    else process.env.STATEFUL_FAKE_LOG = previousLog;
  });

  const handlers = new Map();
  const tools = new Map();
  const pi = {
    setLabel() {},
    on(name, handler) {
      if (!handlers.has(name)) handlers.set(name, []);
      handlers.get(name).push(handler);
    },
    registerTool(tool) {
      tools.set(tool.name, tool);
    },
  };
  const extension = (await import(pathToFileURL(modulePath).href)).default;
  extension(pi);
  return { handlers, tools };
}

async function emitToolCall(handlers, event, ctx) {
  for (const handler of handlers.get('tool_call') || []) {
    const result = await handler(event, ctx);
    if (result !== undefined) return result;
  }
  return undefined;
}

function readLog(logPath) {
  if (!fs.existsSync(logPath)) return [];
  return fs.readFileSync(logPath, 'utf8').trim().split('\n').filter(Boolean).map((line) => JSON.parse(line));
}

test('lazy_write_resume waits for queued wait_id before claiming and applying saved write', async (t) => {
  const dir = tempDir();
  const workspace = path.join(dir, 'workspace');
  fs.mkdirSync(path.join(workspace, 'src'), { recursive: true });
  fs.writeFileSync(path.join(workspace, 'src/app.txt'), 'old\n');
  const logPath = path.join(dir, 'stateful-calls.jsonl');
  const fakeStateful = writeFakeStateful(dir);
  const { handlers, tools } = await loadExtension(t, fakeStateful, logPath);
  const ctx = {
    cwd: workspace,
    workspaceId: 'workspace-1',
    sessionManager: {
      getSessionId: () => '12345678-1234-1234-1234-123456789abc',
      getLeafId: () => 'leaf',
    },
  };

  const blocked = await emitToolCall(
    handlers,
    { toolName: 'write', input: { path: 'src/app.txt', content: 'new\n' } },
    ctx,
  );
  assert.equal(blocked.block, true);
  assert.match(blocked.reason, /Queued lazy write operation_id: wait-queued/);

  const resume = await tools.get('lazy_write_resume').execute(
    'tool-call-1',
    { operation_id: 'wait-queued' },
    undefined,
    undefined,
    ctx,
  );

  assert.equal(resume.isError, false, resume.content[0].text);
  assert.equal(fs.readFileSync(path.join(workspace, 'src/app.txt'), 'utf8'), 'new\n');

  const calls = readLog(logPath);
  const waitIndex = calls.findIndex((call) =>
    (call.args[0] === 'resume' && call.args[1] === 'next')
      || (call.args[0] === 'notifications' && call.args[1] === 'poll')
  );
  const claimIndex = calls.findIndex((call) => call.args[0] === 'reservation' && call.args[1] === 'claim');
  assert.ok(waitIndex >= 0, 'resume should poll/wait for the queued reservation before claiming');
  assert.ok(claimIndex > waitIndex, 'reservation claim must happen only after wait_id is reported reserved');
  assert.equal(calls[claimIndex].args[calls[claimIndex].args.indexOf('--wait-id') + 1], 'wait-queued');
});
