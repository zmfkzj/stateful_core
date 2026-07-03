'use strict';

const path = require('node:path');
const vscode = require('vscode');
const core = require('./lib/core');

const THROTTLE_MS = 5000;

async function activate(context) {
  const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  context.subscriptions.push(status);

  const runtime = core.readRuntimeFile(core.runtimeFilePath());
  if (runtime.state !== 'active') {
    warn(status, 'Stateful dormant: no runtime file');
    return;
  }

  const folders = vscode.workspace.workspaceFolders || [];
  const workspaces = [];
  for (const folder of folders) {
    const repoRoot = folder.uri.fsPath;
    try {
      const probe = await core.postJson(
        runtime,
        '/v1/human/save-check',
        core.saveCheckBody(runtime, '.'),
        1000,
      );
      if (probe.ok) workspaces.push({ folder, repoRoot });
    } catch (error) {
      warn(status, 'Stateful dormant: save gate unavailable');
    }
  }
  if (!workspaces.length) return;

  const lastLowConfidencePost = new Map();
  const observeLow = (document, kind) => {
    const target = documentTarget(workspaces, document);
    if (!target) return;
    const key = `${kind}:${target.relativePath}`;
    const now = Date.now();
    if ((lastLowConfidencePost.get(key) || 0) + THROTTLE_MS > now) return;
    lastLowConfidencePost.set(key, now);
    postObserve(status, runtime, target.repoRoot, {
      path: target.relativePath,
      exists: true,
      kind,
      confidence: 'low',
      source: 'ide',
    });
  };

  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument((document) => observeLow(document, 'presence')),
    vscode.workspace.onDidChangeTextDocument((event) => observeLow(event.document, 'dirty')),
    vscode.workspace.onWillSaveTextDocument((event) => {
      const target = documentTarget(workspaces, event.document);
      if (!target) return;
      void softSaveGate(status, runtime, target, event.document);
    }),
    vscode.workspace.onDidSaveTextDocument((document) => {
      const target = documentTarget(workspaces, document);
      if (!target) return;
      postObserve(status, runtime, target.repoRoot, {
        path: target.relativePath,
        exists: true,
        content_hash: core.contentHash(Buffer.from(document.getText(), 'utf8')),
        kind: 'save',
        confidence: 'high',
        source: 'ide',
      });
    }),
  );
}

async function softSaveGate(status, runtime, target, document) {
  let result;
  try {
    result = await core.postJson(
      runtime,
      '/v1/human/save-check',
      core.saveCheckBody(runtime, target.relativePath),
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
  if (result.body?.decision !== 'warn') return;

  const message = core.renderConflictMessages(result.body).join('\n');
  // ponytail: soft gate; VS Code has already saved by the time the modal resolves.
  const choice = await vscode.window.showWarningMessage(message, { modal: true }, 'Continue', 'Revert file');
  if (choice === 'Revert file') {
    await vscode.commands.executeCommand('workbench.action.files.revert');
  }
  postObserve(status, runtime, target.repoRoot, {
    path: target.relativePath,
    exists: true,
    content_hash: core.contentHash(Buffer.from(document.getText(), 'utf8')),
    kind: 'save',
    confidence: 'high',
    source: 'ide',
    gate: choice === 'Revert file' ? 'reverted' : 'continued',
  });
}

function postObserve(status, runtime, repoRoot, payload) {
  void core
    .postStateful(runtime, '/v1/human/observe', repoRoot, payload, 'human_observe', 5000)
    .then((response) => {
      if (!response.ok) warn(status, `Stateful observe HTTP ${response.status}`);
    })
    .catch(() => warn(status, 'Stateful observe unavailable'));
}

function documentTarget(workspaces, document) {
  if (!document || document.uri.scheme !== 'file') return null;
  for (const workspace of workspaces) {
    const relativePath = path.relative(workspace.repoRoot, document.uri.fsPath);
    if (!relativePath.startsWith('..') && !path.isAbsolute(relativePath)) {
      return { repoRoot: workspace.repoRoot, relativePath: relativePath.split(path.sep).join('/') };
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
