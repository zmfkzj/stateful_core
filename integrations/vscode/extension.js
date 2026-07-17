'use strict';

const crypto = require('node:crypto');
const path = require('node:path');
const vscode = require('vscode');
const core = require('./lib/core');

const THROTTLE_MS = 5000;
const V2_RUNTIME_UNAVAILABLE =
  'Stateful save gate unavailable: runtime identity does not support stateful.v2; update Stateful before saving';

async function activate(context) {
  const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  context.subscriptions.push(status);

  const runtime = core.readRuntimeFile(core.runtimeFilePath());
  if (runtime.state !== 'active') {
    warn(status, 'Stateful dormant: no runtime file');
    return;
  }
  const workspaces = [];
  for (const folder of vscode.workspace.workspaceFolders || []) {
    const identity = core.enabledRepoIdentity(folder.uri.fsPath);
    if (!identity) continue;
    const workspaceId = core.effectiveWorkspaceId(runtime.workspaceId, identity);
    const workspace = {
      root: identity.root,
      workspaceId,
      actorId: `ide-${workspaceId}${workspaceId === runtime.workspaceId ? `-${identity.worktreeId}` : ''}`,
      repoId: identity.repoId,
      worktreeId: identity.worktreeId,
      branch: identity.branch,
    };
    try {
      await runtimeIdentity(runtime, workspace);
    } catch {
      warn(status, V2_RUNTIME_UNAVAILABLE);
      return;
    }
    try {
      const probe = await core.postJson(
        runtime,
        '/v2/human/save-check',
        v2Envelope(workspace, { paths: [] }, 'human_save_check'),
        1000,
      );
      if (probe.ok) workspaces.push({ folder, workspace });
    } catch {
      warn(status, 'Stateful dormant: save gate unavailable');
    }
  }
  if (!workspaces.length) {
    warn(status, 'Stateful dormant: no enabled repository metadata');
    return;
  }

  const lastLowConfidencePost = new Map();
  const observeLow = (document, kind) => {
    const target = documentTarget(workspaces, document);
    if (!target) return;
    const key = `${target.workspace.actorId}:${kind}:${target.relativePath}`;
    const now = Date.now();
    if ((lastLowConfidencePost.get(key) || 0) + THROTTLE_MS > now) return;
    lastLowConfidencePost.set(key, now);
    postObserve(status, runtime, target.workspace, target.relativePath, kind, 'low');
  };

  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument((document) => observeLow(document, 'presence')),
    vscode.workspace.onDidChangeTextDocument((event) => observeLow(event.document, 'dirty')),
    vscode.workspace.onWillSaveTextDocument((event) => {
      const target = documentTarget(workspaces, event.document);
      if (!target) return;
      void softSaveGate(status, runtime, target);
    }),
    vscode.workspace.onDidSaveTextDocument((document) => {
      const target = documentTarget(workspaces, document);
      if (!target) return;
      postObserve(status, runtime, target.workspace, target.relativePath, 'save', 'high');
    }),
  );
}

async function softSaveGate(status, runtime, target) {
  let result;
  try {
    result = await core.postJson(
      runtime,
      '/v2/human/save-check',
      v2Envelope(target.workspace, { paths: [target.relativePath] }, 'human_save_check'),
      250,
    );
  } catch (error) {
    warn(status, 'Stateful save gate unavailable');
    return;
  }
  if (!result.ok) {
    warn(status, `Stateful save gate HTTP ${result.status}`);
    return;
  }
  if (!result.body?.blocked) return;

  const message = result.body.observations
    ?.map((observation) => observation.summary || observation.relative_path)
    .filter(Boolean)
    .join('\n') || 'Stateful reports unreconciled human changes.';
  // ponytail: soft gate; VS Code has already saved by the time the modal resolves.
  const choice = await vscode.window.showWarningMessage(message, { modal: true }, 'Continue', 'Revert file');
  if (choice === 'Revert file') {
    await vscode.commands.executeCommand('workbench.action.files.revert');
  }
  postReconcile(status, runtime, target.workspace, {
    decision: 'ask_user',
    files_reread: [target.relativePath],
    human_change_summary: `VS Code save gate ${choice === 'Revert file' ? 'reverted' : 'continued'}`,
  });
}

function postObserve(status, runtime, workspace, relativePath, kind, confidence) {
  const observation = {
    relative_path: relativePath,
    kind,
    confidence,
    source: 'vscode',
    summary: `VS Code ${kind}`,
  };
  void core
    .postJson(runtime, '/v2/human/observe', v2Envelope(workspace, observation, 'human_observe'), 5000)
    .then((response) => {
      if (!response.ok) warn(status, `Stateful observe HTTP ${response.status}`);
    })
    .catch(() => warn(status, 'Stateful observe unavailable'));
}

function postReconcile(status, runtime, workspace, payload) {
  void core
    .postJson(runtime, '/v2/reconcile/ack', v2Envelope(workspace, payload, 'human_reconcile'), 5000)
    .then((response) => {
      if (!response.ok) warn(status, `Stateful reconcile HTTP ${response.status}`);
    })
    .catch(() => warn(status, 'Stateful reconcile unavailable'));
}

async function runtimeIdentity(runtime, workspace) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 1000);
  try {
    const url = new URL('/v2/runtime/identity', runtime.baseUrl);
    url.search = v2Query(workspace, 'runtime_identity');
    const response = await fetch(url, {
      method: 'GET',
      headers: { authorization: `Bearer ${runtime.token}` },
      signal: controller.signal,
    });
    const text = await response.text();
    let identity;
    try {
      identity = text ? JSON.parse(text) : null;
    } catch {
      identity = null;
    }
    if (
      !response.ok ||
      identity?.protocol_version !== 'stateful.v2' ||
      identity.journal_schema_version !== 2 ||
      !identity.capabilities?.includes('presence')
    ) {
      throw new Error('unsupported runtime identity');
    }
  } finally {
    clearTimeout(timeout);
  }
}

function v2Query(workspace, event) {
  const agentId = workspace.actorId;
  return new URLSearchParams({
    protocol_version: 'stateful.v2',
    request_id: crypto.randomUUID(),
    observed_at: new Date().toISOString(),
    agent_id: agentId,
    actor_id: agentId,
    actor_type: 'human',
    root: workspace.root,
    workspace_id: workspace.workspaceId,
    repo_id: workspace.repoId,
    worktree_id: workspace.worktreeId,
    branch: workspace.branch,
    kind: 'ide',
    event,
    source_ref: 'stateful.vscode',
  }).toString();
}

function v2Envelope(workspace, payload, event) {
  const agentId = workspace.actorId;
  return {
    protocol_version: 'stateful.v2',
    request_id: crypto.randomUUID(),
    observed_at: new Date().toISOString(),
    agent: { agent_id: agentId, actor_id: agentId, actor_type: 'human' },
    workspace: {
      root: workspace.root,
      workspace_id: workspace.workspaceId,
      repo_id: workspace.repoId,
      worktree_id: workspace.worktreeId,
      branch: workspace.branch,
    },
    source: { kind: 'ide', event, source_ref: 'stateful.vscode' },
    payload,
  };
}

function documentTarget(workspaces, document) {
  if (!document || document.uri.scheme !== 'file') return null;
  for (const workspace of workspaces) {
    const relativePath = path.relative(workspace.workspace.root, document.uri.fsPath);
    if (!relativePath.startsWith('..') && !path.isAbsolute(relativePath)) {
      return { workspace: workspace.workspace, relativePath: relativePath.split(path.sep).join('/') };
    }
  }
  return null;
}

function warn(status, text) {
  status.text = text;
  status.show();
}

function deactivate() {}

module.exports = { activate, deactivate };
