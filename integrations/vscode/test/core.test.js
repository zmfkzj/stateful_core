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

function vscodeHarness(workspaceFolders = [{ uri: { fsPath: '/repo' } }]) {
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
      workspaceFolders,
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

function writeRuntime(home, workspaceId = 'workspace-1') {
  const runtimeDir = path.join(home, 'runtime');
  fs.mkdirSync(runtimeDir, { recursive: true });
  fs.writeFileSync(
    path.join(runtimeDir, 'server.json'),
    JSON.stringify({
      base_url: 'http://stateful.test',
      token: 'secret-token',
      workspace_id: workspaceId,
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

function writeGitRepo(root, branch = 'main') {
  fs.mkdirSync(path.join(root, '.git'), { recursive: true });
  fs.writeFileSync(path.join(root, '.git', 'HEAD'), `ref: refs/heads/${branch}\n`);
}

function writeEnabledRepos(home, entries) {
  fs.writeFileSync(
    path.join(home, 'config.yml'),
    `repos:\n${entries.map((entry) => [
      `- repo_id: ${entry.repoId}`,
      `  root: ${entry.root}`,
      `  enabled: ${entry.enabled === false ? 'false' : 'true'}`,
      '  enabled_at: "0"',
      `  policy_config_path: ${path.join(entry.root, '.stateful', 'config.yml')}`,
      '  allowed_tools:',
      '  - task',
    ].join('\n')).join('\n')}\n`,
  );
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

test('enabledRepoIdentity reads worktree HEAD metadata from installed config', () => {
  const home = tempDir();
  const root = fs.realpathSync(tempDir());
  const gitdir = tempDir();
  fs.writeFileSync(path.join(root, '.git'), `gitdir: ${gitdir}\n`);
  fs.writeFileSync(path.join(gitdir, 'HEAD'), 'ref: refs/heads/feature/worktree\n');
  writeEnabledRepos(home, [{ repoId: 'repo-worktree', root }]);

  assert.deepStrictEqual(
    core().enabledRepoIdentity(root, { STATEFUL_HOME: home }),
    {
      root,
      repoId: 'repo-worktree',
      worktreeId: 'repo-worktree',
      branch: 'feature/worktree',
    },
  );
});

test('effectiveWorkspaceId matches Rust local runtime derivation', () => {
  const identity = { worktreeId: 'repo-worktree' };
  for (const runtimeWorkspaceId of ['local', 'shared', 'unknown']) {
    assert.equal(core().effectiveWorkspaceId(runtimeWorkspaceId, identity), 'workspace-worktree');
  }
  assert.equal(core().effectiveWorkspaceId('remote-workspace', identity), 'remote-workspace');
});

test('enabledRepoIdentity accepts YAML boolean comments but rejects quoted booleans', () => {
  const home = tempDir();
  const root = fs.realpathSync(tempDir());
  writeGitRepo(root);
  writeEnabledRepos(home, [{ repoId: 'repo-bool', root }]);
  const configPath = path.join(home, 'config.yml');

  fs.writeFileSync(configPath, fs.readFileSync(configPath, 'utf8').replace('enabled: true', 'enabled: "true"'));
  assert.equal(core().enabledRepoIdentity(root, { STATEFUL_HOME: home }), null);

  fs.writeFileSync(configPath, fs.readFileSync(configPath, 'utf8').replace('enabled: "true"', 'enabled: true # enabled'));
  assert.equal(core().enabledRepoIdentity(root, { STATEFUL_HOME: home }).repoId, 'repo-bool');
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

function strictRuntimeIdentityResponse(url, options, required) {
  const request = new URL(String(url));
  assert.equal(request.pathname, '/v2/runtime/identity');
  assert.equal(options.method, 'GET');
  assert.deepStrictEqual(options.headers, { authorization: 'Bearer secret-token' });

  const query = Object.fromEntries(request.searchParams);
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
  const repo = tempDir();
  const root = fs.realpathSync(repo);
  const originalHome = process.env.STATEFUL_HOME;
  const originalFetch = global.fetch;
  writeGitRepo(root);
  writeRuntime(home);
  writeEnabledRepos(home, [{ repoId: 'repo-query', root }]);
  process.env.STATEFUL_HOME = home;
  const required = {
    protocol_version: 'stateful.v2',
    agent_id: 'ide-workspace-1-repo-query',
    actor_id: 'ide-workspace-1-repo-query',
    actor_type: 'human',
    root,
    workspace_id: 'workspace-1',
    repo_id: 'repo-query',
    worktree_id: 'repo-query',
    branch: 'main',
    kind: 'ide',
    event: 'runtime_identity',
    source_ref: 'stateful.vscode',
  };
  global.fetch = async (url, options = {}) =>
    new URL(String(url)).pathname === '/v2/runtime/identity'
      ? strictRuntimeIdentityResponse(url, options, required)
      : jsonResponse({ blocked: false, observations: [] });

  try {
    const harness = vscodeHarness([{ uri: { fsPath: root } }]);
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
  const repo = tempDir();
  const root = fs.realpathSync(repo);
  const originalHome = process.env.STATEFUL_HOME;
  const originalFetch = global.fetch;
  const calls = [];
  writeGitRepo(root);
  writeRuntime(home);
  writeEnabledRepos(home, [{ repoId: 'repo-envelope', root }]);
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
    const harness = vscodeHarness([{ uri: { fsPath: root } }]);
    await extension(harness.api).activate({ subscriptions: { push() {} } });
    harness.handlers.open({
      uri: { scheme: 'file', fsPath: path.join(root, 'src', 'app.js') },
      getText: () => 'const answer = 42;\n',
    });
    await settle();
    harness.handlers.willSave({
      document: {
        uri: { scheme: 'file', fsPath: path.join(root, 'src', 'app.js') },
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
      agent: { agent_id: 'ide-workspace-1-repo-envelope', actor_id: 'ide-workspace-1-repo-envelope', actor_type: 'human' },
      workspace: {
        root,
        workspace_id: 'workspace-1',
        repo_id: 'repo-envelope',
        worktree_id: 'repo-envelope',
        branch: 'main',
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
  const repo = tempDir();
  const root = fs.realpathSync(repo);
  const originalHome = process.env.STATEFUL_HOME;
  const originalFetch = global.fetch;
  writeGitRepo(root);
  writeRuntime(home);
  writeEnabledRepos(home, [{ repoId: 'repo-unsupported', root }]);
  process.env.STATEFUL_HOME = home;
  global.fetch = async () => jsonResponse({ protocol_version: 'unsupported' });

  try {
    const harness = vscodeHarness([{ uri: { fsPath: root } }]);
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
  const repo = tempDir();
  const root = fs.realpathSync(repo);
  const originalHome = process.env.STATEFUL_HOME;
  const originalFetch = global.fetch;
  writeGitRepo(root);
  writeRuntime(home);
  writeEnabledRepos(home, [{ repoId: 'repo-missing', root }]);
  process.env.STATEFUL_HOME = home;
  global.fetch = async () => jsonResponse({});

  try {
    const harness = vscodeHarness([{ uri: { fsPath: root } }]);
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

test('VS Code derives enabled local repository identity for runtime and human envelopes', async () => {
  const home = tempDir();
  const repo = tempDir();
  const root = fs.realpathSync(repo);
  const originalHome = process.env.STATEFUL_HOME;
  const originalFetch = global.fetch;
  const calls = [];
  writeGitRepo(root, 'feature/stateful');
  writeRuntime(home, 'local');
  writeEnabledRepos(home, [{ repoId: 'repo-primary', root }]);
  process.env.STATEFUL_HOME = home;
  global.fetch = async (url, options = {}) => {
    const call = { url: String(url), options };
    calls.push(call);
    const request = new URL(call.url);
    if (request.pathname === '/v2/runtime/identity') {
      const query = Object.fromEntries(request.searchParams);
      const expected = {
        agent_id: 'ide-workspace-primary',
        actor_id: 'ide-workspace-primary',
        actor_type: 'human',
        root,
        workspace_id: 'workspace-primary',
        repo_id: 'repo-primary',
        worktree_id: 'repo-primary',
        branch: 'feature/stateful',
      };
      return Object.entries(expected).every(([field, value]) => query[field] === value)
        ? jsonResponse({ protocol_version: 'stateful.v2', journal_schema_version: 2, capabilities: ['presence'] })
        : jsonResponse({ error: 'incorrect repository identity' }, 400);
    }
    return jsonResponse({ blocked: false, observations: [] });
  };

  try {
    const harness = vscodeHarness([{ uri: { fsPath: root } }]);
    await extension(harness.api).activate({ subscriptions: { push() {} } });
    assert.equal(typeof harness.handlers.willSave, 'function');
    harness.handlers.open({
      uri: { scheme: 'file', fsPath: path.join(root, 'src', 'app.js') },
      getText: () => 'const answer = 42;\n',
    });
    await settle();
    harness.handlers.willSave({
      document: {
        uri: { scheme: 'file', fsPath: path.join(root, 'src', 'app.js') },
        getText: () => 'const answer = 42;\n',
      },
    });
    await settle();

    const envelopes = calls
      .filter((call) => call.options.method === 'POST')
      .map((call) => JSON.parse(call.options.body));
    assert.ok(envelopes.length >= 3);
    for (const envelope of envelopes) {
      assert.deepStrictEqual(envelope.agent, {
        agent_id: 'ide-workspace-primary',
        actor_id: 'ide-workspace-primary',
        actor_type: 'human',
      });
      assert.deepStrictEqual(envelope.workspace, {
        root,
        workspace_id: 'workspace-primary',
        repo_id: 'repo-primary',
        worktree_id: 'repo-primary',
        branch: 'feature/stateful',
      });
    }
  } finally {
    global.fetch = originalFetch;
    if (originalHome === undefined) delete process.env.STATEFUL_HOME;
    else process.env.STATEFUL_HOME = originalHome;
  }
});

test('VS Code preserves an explicit non-local runtime workspace ID', async () => {
  const home = tempDir();
  const repo = tempDir();
  const root = fs.realpathSync(repo);
  const originalHome = process.env.STATEFUL_HOME;
  const originalFetch = global.fetch;
  const calls = [];
  writeGitRepo(root);
  writeRuntime(home, 'remote-workspace');
  writeEnabledRepos(home, [{ repoId: 'repo-explicit', root }]);
  process.env.STATEFUL_HOME = home;
  global.fetch = async (url, options = {}) => {
    const call = { url: String(url), options };
    calls.push(call);
    if (new URL(call.url).pathname === '/v2/runtime/identity') {
      const query = Object.fromEntries(new URL(call.url).searchParams);
      return query.workspace_id === 'remote-workspace'
        && query.agent_id === 'ide-remote-workspace-repo-explicit'
        && query.repo_id === 'repo-explicit'
        && query.worktree_id === 'repo-explicit'
        ? jsonResponse({ protocol_version: 'stateful.v2', journal_schema_version: 2, capabilities: ['presence'] })
        : jsonResponse({ error: 'incorrect explicit workspace identity' }, 400);
    }
    return jsonResponse({ blocked: false, observations: [] });
  };

  try {
    const harness = vscodeHarness([{ uri: { fsPath: root } }]);
    await extension(harness.api).activate({ subscriptions: { push() {} } });
    assert.equal(typeof harness.handlers.willSave, 'function');
    const probe = calls.find((call) => new URL(call.url).pathname === '/v2/human/save-check');
    const envelope = JSON.parse(probe.options.body);
    assert.equal(envelope.workspace.workspace_id, 'remote-workspace');
    assert.equal(envelope.agent.agent_id, 'ide-remote-workspace-repo-explicit');
  } finally {
    global.fetch = originalFetch;
    if (originalHome === undefined) delete process.env.STATEFUL_HOME;
    else process.env.STATEFUL_HOME = originalHome;
  }
});

test('VS Code leaves disabled, malformed, and mismatched installed metadata dormant without posts', async () => {
  const home = tempDir();
  const disabledRoot = fs.realpathSync(tempDir());
  const mismatchedRoot = fs.realpathSync(tempDir());
  const staleRoot = path.join(home, 'stale-root');
  const originalHome = process.env.STATEFUL_HOME;
  const originalFetch = global.fetch;
  const calls = [];
  writeGitRepo(disabledRoot);
  writeGitRepo(mismatchedRoot);
  writeRuntime(home, 'local');
  writeEnabledRepos(home, [
    { repoId: 'repo-disabled', root: disabledRoot, enabled: false },
    { repoId: 'repo-stale', root: staleRoot },
  ]);
  process.env.STATEFUL_HOME = home;
  global.fetch = async (url, options = {}) => {
    calls.push({ url: String(url), options });
    return jsonResponse({ protocol_version: 'stateful.v2', journal_schema_version: 2, capabilities: ['presence'] });
  };

  try {
    const harness = vscodeHarness([
      { uri: { fsPath: disabledRoot } },
      { uri: { fsPath: mismatchedRoot } },
    ]);
    await extension(harness.api).activate({ subscriptions: { push() {} } });
    assert.deepStrictEqual(calls, []);
    assert.equal(harness.handlers.willSave, undefined);
    fs.writeFileSync(path.join(home, 'config.yml'), 'repos:\n- repo_id: \n');
    const malformedHarness = vscodeHarness([{ uri: { fsPath: mismatchedRoot } }]);
    await extension(malformedHarness.api).activate({ subscriptions: { push() {} } });
    assert.deepStrictEqual(calls, []);
    assert.equal(malformedHarness.handlers.willSave, undefined);
  } finally {
    global.fetch = originalFetch;
    if (originalHome === undefined) delete process.env.STATEFUL_HOME;
    else process.env.STATEFUL_HOME = originalHome;
  }
});

test('VS Code keeps multi-root repository envelopes and low-confidence actors distinct', async () => {
  const home = tempDir();
  const firstRoot = fs.realpathSync(tempDir());
  const secondRoot = fs.realpathSync(tempDir());
  const originalHome = process.env.STATEFUL_HOME;
  const originalFetch = global.fetch;
  const calls = [];
  writeGitRepo(firstRoot, 'first');
  writeGitRepo(secondRoot, 'second');
  writeRuntime(home, 'remote-workspace');
  writeEnabledRepos(home, [
    { repoId: 'repo-first', root: firstRoot },
    { repoId: 'repo-second', root: secondRoot },
  ]);
  process.env.STATEFUL_HOME = home;
  global.fetch = async (url, options = {}) => {
    calls.push({ url: String(url), options });
    return jsonResponse({ protocol_version: 'stateful.v2', journal_schema_version: 2, capabilities: ['presence'], blocked: false, observations: [] });
  };

  try {
    const harness = vscodeHarness([
      { uri: { fsPath: firstRoot } },
      { uri: { fsPath: secondRoot } },
    ]);
    await extension(harness.api).activate({ subscriptions: { push() {} } });
    harness.handlers.open({
      uri: { scheme: 'file', fsPath: path.join(firstRoot, 'src', 'app.js') },
      getText: () => '',
    });
    harness.handlers.open({
      uri: { scheme: 'file', fsPath: path.join(secondRoot, 'src', 'app.js') },
      getText: () => '',
    });
    await settle();

    const identities = calls
      .filter((call) => call.options.method === 'POST')
      .map((call) => JSON.parse(call.options.body))
      .filter((envelope) => envelope.source.event === 'human_observe')
      .map((envelope) => ({
        agentId: envelope.agent.agent_id,
        workspaceId: envelope.workspace.workspace_id,
        repoId: envelope.workspace.repo_id,
        root: envelope.workspace.root,
      }))
      .sort((left, right) => left.repoId.localeCompare(right.repoId));
    assert.deepStrictEqual(identities, [
      {
        agentId: 'ide-remote-workspace-repo-first',
        workspaceId: 'remote-workspace',
        repoId: 'repo-first',
        root: firstRoot,
      },
      {
        agentId: 'ide-remote-workspace-repo-second',
        workspaceId: 'remote-workspace',
        repoId: 'repo-second',
        root: secondRoot,
      },
    ]);
  } finally {
    global.fetch = originalFetch;
    if (originalHome === undefined) delete process.env.STATEFUL_HOME;
    else process.env.STATEFUL_HOME = originalHome;
  }
});

test('VS Code accepts document paths through a symlinked enabled folder', async () => {
  const home = tempDir();
  const root = fs.realpathSync(tempDir());
  const link = path.join(tempDir(), 'linked-repo');
  const originalHome = process.env.STATEFUL_HOME;
  const originalFetch = global.fetch;
  const calls = [];
  writeGitRepo(root);
  fs.symlinkSync(root, link, 'dir');
  writeRuntime(home, 'local');
  writeEnabledRepos(home, [{ repoId: 'repo-link', root }]);
  process.env.STATEFUL_HOME = home;
  global.fetch = async (url, options = {}) => {
    calls.push({ url: String(url), options });
    return jsonResponse({ protocol_version: 'stateful.v2', journal_schema_version: 2, capabilities: ['presence'], blocked: false, observations: [] });
  };

  try {
    const harness = vscodeHarness([{ uri: { fsPath: link } }]);
    await extension(harness.api).activate({ subscriptions: { push() {} } });
    harness.handlers.open({
      uri: { scheme: 'file', fsPath: path.join(link, 'src', 'app.js') },
      getText: () => '',
    });
    await settle();
    const observation = calls
      .filter((call) => new URL(call.url).pathname === '/v2/human/observe')
      .map((call) => JSON.parse(call.options.body))
      .at(-1);
    assert.equal(observation.workspace.root, root);
    assert.equal(observation.payload.relative_path, 'src/app.js');
  } finally {
    global.fetch = originalFetch;
    if (originalHome === undefined) delete process.env.STATEFUL_HOME;
    else process.env.STATEFUL_HOME = originalHome;
  }
});

test('VS Code routes nested enabled roots to the most specific folder', async () => {
  const home = tempDir();
  const outerRoot = fs.realpathSync(tempDir());
  const innerRoot = path.join(outerRoot, 'packages', 'inner');
  const originalHome = process.env.STATEFUL_HOME;
  const originalFetch = global.fetch;
  const calls = [];
  fs.mkdirSync(innerRoot, { recursive: true });
  writeGitRepo(outerRoot);
  writeGitRepo(innerRoot);
  writeRuntime(home, 'local');
  writeEnabledRepos(home, [
    { repoId: 'repo-outer', root: outerRoot },
    { repoId: 'repo-inner', root: innerRoot },
  ]);
  process.env.STATEFUL_HOME = home;
  global.fetch = async (url, options = {}) => {
    calls.push({ url: String(url), options });
    return jsonResponse({ protocol_version: 'stateful.v2', journal_schema_version: 2, capabilities: ['presence'], blocked: false, observations: [] });
  };

  try {
    const harness = vscodeHarness([
      { uri: { fsPath: outerRoot } },
      { uri: { fsPath: innerRoot } },
    ]);
    await extension(harness.api).activate({ subscriptions: { push() {} } });
    harness.handlers.open({
      uri: { scheme: 'file', fsPath: path.join(innerRoot, 'src', 'app.js') },
      getText: () => '',
    });
    await settle();
    const observation = calls
      .filter((call) => new URL(call.url).pathname === '/v2/human/observe')
      .map((call) => JSON.parse(call.options.body))
      .at(-1);
    assert.equal(observation.workspace.repo_id, 'repo-inner');
    assert.equal(observation.payload.relative_path, 'src/app.js');
  } finally {
    global.fetch = originalFetch;
    if (originalHome === undefined) delete process.env.STATEFUL_HOME;
    else process.env.STATEFUL_HOME = originalHome;
  }
});
