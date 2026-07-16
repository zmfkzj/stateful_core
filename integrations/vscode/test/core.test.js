const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');
const Module = require('node:module');

function jsonResponse(body, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => JSON.stringify(body),
  };
}

function vscodeHarness() {
  const handlers = {};
  const status = {
    text: '',
    showCalls: 0,
    show() {
      this.showCalls += 1;
    },
  };
  const api = {
    StatusBarAlignment: { Left: 1 },
    window: {
      createStatusBarItem: () => status,
      showWarningMessage: async () => 'Continue',
    },
    workspace: {
      workspaceFolders: [{ uri: { fsPath: '/repo' } }],
      onDidOpenTextDocument: (handler) => {
        handlers.open = handler;
        return { dispose() {} };
      },
      onDidChangeTextDocument: (handler) => {
        handlers.change = handler;
        return { dispose() {} };
      },
      onWillSaveTextDocument: (handler) => {
        handlers.willSave = handler;
        return { dispose() {} };
      },
      onDidSaveTextDocument: (handler) => {
        handlers.didSave = handler;
        return { dispose() {} };
      },
    },
    commands: { executeCommand: async () => {} },
  };
  return { api, handlers, status };
}

function extension(vscode) {
  const originalLoad = Module._load;
  Module._load = function load(request, parent, isMain) {
    return request === 'vscode' ? vscode : originalLoad.call(this, request, parent, isMain);
  };
  delete require.cache[require.resolve('../extension.js')];
  try {
    return require('../extension.js');
  } finally {
    Module._load = originalLoad;
  }
}

function writeRuntime(home) {
  const runtimeDir = path.join(home, 'runtime');
  fs.mkdirSync(runtimeDir, { recursive: true });
  fs.writeFileSync(
    path.join(runtimeDir, 'server.json'),
    JSON.stringify({
      base_url: 'http://stateful.test',
      token: 'secret-token',
      workspace_id: 'workspace-1',
    }),
  );
}

async function settle() {
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
}

function core() {
  return require('../lib/core.js');
}

function tempDir() {
  fs.mkdirSync(os.tmpdir(), { recursive: true });
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

test('VS Code integration has no V1 transport surface', () => {
  const legacyProtocol = ['stateful', 'v1'].join('.');
  const legacyPath = `/${legacyProtocol.slice(-2)}/`;
  const retiredExports = [
    ['make', 'Envelope'].join(''),
    ['post', 'Stateful'].join(''),
    ['save', 'CheckBody'].join(''),
  ];

  for (const file of ['../extension.js', '../lib/core.js', './core.test.js']) {
    const source = fs.readFileSync(path.join(__dirname, file), 'utf8');
    assert.equal(source.includes(legacyProtocol), false, file);
    assert.equal(source.includes(legacyPath), false, file);
  }
  assert.deepStrictEqual(retiredExports.filter((name) => name in core()), []);
});

function v2Envelope(call) {
  const body = JSON.parse(call.options.body);
  assert.match(body.request_id, /^[0-9a-f-]{36}$/);
  assert.ok(Number.isFinite(Date.parse(body.observed_at)));
  delete body.request_id;
  delete body.observed_at;
  return body;
}

function strictRuntimeIdentityResponse(url, options) {
  const request = new URL(String(url));
  assert.equal(request.pathname, '/v2/runtime/identity');
  assert.equal(options.method, 'GET');
  assert.deepStrictEqual(options.headers, { authorization: 'Bearer secret-token' });

  const query = Object.fromEntries(request.searchParams);
  const required = {
    protocol_version: 'stateful.v2',
    agent_id: 'ide-workspace-1',
    actor_id: 'ide-workspace-1',
    actor_type: 'human',
    root: '/repo',
    workspace_id: 'workspace-1',
    repo_id: 'unknown',
    worktree_id: 'unknown',
    branch: 'unknown',
    kind: 'ide',
    event: 'runtime_identity',
    source_ref: 'stateful.vscode',
  };
  for (const [field, expected] of Object.entries(required)) {
    if (query[field] !== expected) {
      return jsonResponse({ error: `missing or invalid ${field}` }, 400);
    }
  }
  assert.match(query.request_id, /^[0-9a-f-]{36}$/);
  assert.ok(Number.isFinite(Date.parse(query.observed_at)));
  return jsonResponse({
    protocol_version: 'stateful.v2',
    journal_schema_version: 2,
    capabilities: ['presence'],
  });
}

test('VS Code sends a complete V2 query envelope to the runtime identity route', async () => {
  const home = tempDir();
  const originalHome = process.env.STATEFUL_HOME;
  const originalFetch = global.fetch;
  writeRuntime(home);
  process.env.STATEFUL_HOME = home;
  global.fetch = async (url, options = {}) =>
    new URL(String(url)).pathname === '/v2/runtime/identity'
      ? strictRuntimeIdentityResponse(url, options)
      : jsonResponse({ blocked: false, observations: [] });

  try {
    const harness = vscodeHarness();
    await extension(harness.api).activate({ subscriptions: { push() {} } });

    assert.equal(typeof harness.handlers.willSave, 'function');
  } finally {
    global.fetch = originalFetch;
    if (originalHome === undefined) delete process.env.STATEFUL_HOME;
    else process.env.STATEFUL_HOME = originalHome;
  }
});

test('VS Code uses the V2 runtime handshake and human request envelopes', async () => {
  const home = tempDir();
  const originalHome = process.env.STATEFUL_HOME;
  const originalFetch = global.fetch;
  const calls = [];
  writeRuntime(home);
  process.env.STATEFUL_HOME = home;
  global.fetch = async (url, options = {}) => {
    const call = { url: String(url), options };
    calls.push(call);
    if (new URL(call.url).pathname === '/v2/runtime/identity') {
      return jsonResponse({
        protocol_version: 'stateful.v2',
        journal_schema_version: 2,
        capabilities: ['presence'],
      });
    }
    if (new URL(call.url).pathname === '/v2/human/save-check') {
      const saveChecks = calls.filter(
        (candidate) => new URL(candidate.url).pathname === '/v2/human/save-check',
      );
      return jsonResponse(saveChecks.length === 1 ? { blocked: false, observations: [] } : {
        blocked: true,
        observations: [{ relative_path: 'src/app.js', summary: 'human save' }],
      });
    }
    return jsonResponse({});
  };

  try {
    const harness = vscodeHarness();
    await extension(harness.api).activate({ subscriptions: { push() {} } });
    harness.handlers.open({
      uri: { scheme: 'file', fsPath: '/repo/src/app.js' },
      getText: () => 'const answer = 42;\n',
    });
    await settle();
    harness.handlers.willSave({
      document: {
        uri: { scheme: 'file', fsPath: '/repo/src/app.js' },
        getText: () => 'const answer = 42;\n',
      },
    });
    await settle();

    assert.deepStrictEqual(calls.map((call) => new URL(call.url).pathname), [
      '/v2/runtime/identity',
      '/v2/human/save-check',
      '/v2/human/observe',
      '/v2/human/save-check',
      '/v2/reconcile/ack',
    ]);
    assert.equal(calls[0].options.method, 'GET');
    assert.deepStrictEqual(calls[0].options.headers, { authorization: 'Bearer secret-token' });

    const identity = {
      agent: { agent_id: 'ide-workspace-1', actor_id: 'ide-workspace-1', actor_type: 'human' },
      workspace: {
        root: '/repo',
        workspace_id: 'workspace-1',
        repo_id: 'unknown',
        worktree_id: 'unknown',
        branch: 'unknown',
      },
    };
    assert.deepStrictEqual(v2Envelope(calls[1]), {
      protocol_version: 'stateful.v2',
      ...identity,
      source: { kind: 'ide', event: 'human_save_check', source_ref: 'stateful.vscode' },
      payload: { paths: [] },
    });
    assert.deepStrictEqual(v2Envelope(calls[2]), {
      protocol_version: 'stateful.v2',
      ...identity,
      source: { kind: 'ide', event: 'human_observe', source_ref: 'stateful.vscode' },
      payload: {
        relative_path: 'src/app.js',
        kind: 'presence',
        confidence: 'low',
        source: 'vscode',
        summary: 'VS Code presence',
      },
    });
    assert.deepStrictEqual(v2Envelope(calls[3]), {
      protocol_version: 'stateful.v2',
      ...identity,
      source: { kind: 'ide', event: 'human_save_check', source_ref: 'stateful.vscode' },
      payload: { paths: ['src/app.js'] },
    });
    assert.deepStrictEqual(v2Envelope(calls[4]), {
      protocol_version: 'stateful.v2',
      ...identity,
      source: { kind: 'ide', event: 'human_reconcile', source_ref: 'stateful.vscode' },
      payload: {
        decision: 'ask_user',
        files_reread: ['src/app.js'],
        human_change_summary: 'VS Code save gate continued',
      },
    });
  } finally {
    global.fetch = originalFetch;
    if (originalHome === undefined) delete process.env.STATEFUL_HOME;
    else process.env.STATEFUL_HOME = originalHome;
  }
});

test('VS Code fails save checks conservatively when V2 runtime identity is unsupported', async () => {
  const home = tempDir();
  const originalHome = process.env.STATEFUL_HOME;
  const originalFetch = global.fetch;
  writeRuntime(home);
  process.env.STATEFUL_HOME = home;
  global.fetch = async () => jsonResponse({ protocol_version: 'unsupported' });

  try {
    const harness = vscodeHarness();
    await extension(harness.api).activate({ subscriptions: { push() {} } });

    assert.equal(
      harness.status.text,
      'Stateful save gate unavailable: runtime identity does not support stateful.v2; update Stateful before saving',
    );
    assert.equal(harness.status.showCalls, 1);
    assert.equal(harness.handlers.willSave, undefined);
  } finally {
    global.fetch = originalFetch;
    if (originalHome === undefined) delete process.env.STATEFUL_HOME;
    else process.env.STATEFUL_HOME = originalHome;
  }
});

test('VS Code fails save checks conservatively when V2 runtime identity is missing', async () => {
  const home = tempDir();
  const originalHome = process.env.STATEFUL_HOME;
  const originalFetch = global.fetch;
  writeRuntime(home);
  process.env.STATEFUL_HOME = home;
  global.fetch = async () => jsonResponse({});

  try {
    const harness = vscodeHarness();
    await extension(harness.api).activate({ subscriptions: { push() {} } });

    assert.equal(
      harness.status.text,
      'Stateful save gate unavailable: runtime identity does not support stateful.v2; update Stateful before saving',
    );
    assert.equal(harness.status.showCalls, 1);
    assert.equal(harness.handlers.willSave, undefined);
  } finally {
    global.fetch = originalFetch;
    if (originalHome === undefined) delete process.env.STATEFUL_HOME;
    else process.env.STATEFUL_HOME = originalHome;
  }
});
