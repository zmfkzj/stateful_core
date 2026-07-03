const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');

function core() {
  return require('../lib/core.js');
}

function tempDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'stateful-vscode-core-'));
}

test('readRuntimeFile parses server runtime discovery into active connection config', () => {
  const dir = tempDir();
  const runtimeFile = path.join(dir, 'server.json');
  fs.writeFileSync(
    runtimeFile,
    JSON.stringify({
      base_url: 'http://127.0.0.1:43873',
      token: 'secret-token',
      workspace_id: 'workspace-1',
    }),
  );

  assert.deepStrictEqual(core().readRuntimeFile(runtimeFile), {
    state: 'active',
    baseUrl: 'http://127.0.0.1:43873',
    token: 'secret-token',
    workspaceId: 'workspace-1',
  });
});

test('readRuntimeFile treats a missing runtime file as dormant fail-open state', () => {
  const missingRuntimeFile = path.join(tempDir(), 'runtime', 'server.json');

  assert.deepStrictEqual(core().readRuntimeFile(missingRuntimeFile), { state: 'dormant' });
});

test('saveCheckBody matches server top-level request shape', () => {
  assert.deepStrictEqual(core().saveCheckBody({ workspaceId: 'workspace-1' }, 'src/app.ts'), {
    workspace_id: 'workspace-1',
    paths: ['src/app.ts'],
  });
});

test('contentHash matches server fnv1a64 format for observed file bytes', () => {
  assert.equal(core().contentHash(Buffer.from('hello\n')), 'fnv1a64:a9bc80cca21f28b3');
});

test('sha256Hex returns lowercase hex digest', () => {
  assert.equal(
    core().sha256Hex(Buffer.from('hello\n')),
    '5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03',
  );
});

test('renderConflictMessages preserves denial message and conflict details', () => {
  assert.deepStrictEqual(
    core().renderConflictMessages({
      message: 'write denied',
      conflicts: [
        {
          severity: 'block',
          reason: 'active claim',
          target_resource: 'file:src/app.ts',
          conflicting_agent_id: 'agent-b',
        },
      ],
    }),
    ['write denied', 'block: active claim (file:src/app.ts; conflicting agent agent-b)'],
  );
});
