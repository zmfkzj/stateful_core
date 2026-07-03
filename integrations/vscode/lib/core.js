'use strict';

const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');

function runtimeFilePath(env = process.env, home = os.homedir()) {
  const statefulHome = env.STATEFUL_HOME || path.join(home, '.stateful_core');
  return path.join(statefulHome, 'runtime', 'server.json');
}

function readRuntimeFile(filePath = runtimeFilePath()) {
  let data;
  try {
    data = JSON.parse(fs.readFileSync(filePath, 'utf8'));
  } catch (error) {
    if (error && error.code === 'ENOENT') return { state: 'dormant' };
    return { state: 'dormant', reason: 'invalid_runtime' };
  }

  const baseUrl = data.base_url || data.url;
  const token = data.token;
  const workspaceId = data.workspace_id || data.workspaceId;
  if (!baseUrl || !token || !workspaceId) {
    return { state: 'dormant', reason: 'invalid_runtime' };
  }
  return { state: 'active', baseUrl, token, workspaceId };
}

function contentHash(bytes) {
  const input = Buffer.isBuffer(bytes) ? bytes : Buffer.from(String(bytes));
  let hash = 0xcbf29ce484222325n;
  for (const byte of input) {
    hash ^= BigInt(byte);
    hash = BigInt.asUintN(64, hash * 0x100000001b3n);
  }
  return `fnv1a64:${hash.toString(16).padStart(16, '0')}`;
}

function sha256Hex(bytes) {
  return crypto.createHash('sha256').update(bytes).digest('hex');
}

function renderConflictMessages(response) {
  const messages = [];
  if (response && response.message) messages.push(response.message);
  for (const conflict of response?.conflicts || []) {
    const severity = conflict.severity || conflict.kind || 'conflict';
    const reason = conflict.reason || conflict.purpose || 'active coordination';
    const target = conflict.target_resource || conflict.path || conflict.resource;
    const agent = conflict.conflicting_agent_id || conflict.agent_id;
    const expires = conflict.expires_at ? `; expires ${conflict.expires_at}` : '';
    const parts = [target, agent && `conflicting agent ${agent}`].filter(Boolean).join('; ');
    messages.push(`${severity}: ${reason}${parts ? ` (${parts}${expires})` : expires}`);
  }
  return messages.length ? messages : ['Stateful reports possible save conflicts.'];
}

function makeEnvelope(runtime, repoRoot, payload, event, sourceKind = 'ide') {
  const workspaceId = runtime.workspaceId;
  const agentId = `${sourceKind}-${workspaceId}`;
  return {
    protocol_version: 'stateful.v1',
    request_id: crypto.randomUUID(),
    observed_at: new Date().toISOString(),
    agent: {
      agent_id: agentId,
      actor_id: agentId,
      actor_type: 'human',
    },
    workspace: {
      root: repoRoot || '',
      workspace_id: workspaceId,
      repo_id: '',
      worktree_id: '',
      branch: '',
    },
    source: {
      kind: sourceKind,
      event,
      source_ref: 'stateful.vscode',
    },
    payload,
  };
}


function saveCheckBody(runtime, relativePath) {
  return {
    workspace_id: runtime.workspaceId,
    paths: [relativePath],
  };
}
async function postJson(runtime, endpointPath, body, timeoutMs = 5000) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(new URL(endpointPath, runtime.baseUrl), {
      method: 'POST',
      headers: {
        authorization: `Bearer ${runtime.token}`,
        'content-type': 'application/json',
      },
      body: JSON.stringify(body),
      signal: controller.signal,
    });
    const text = await response.text();
    let json;
    try {
      json = text ? JSON.parse(text) : null;
    } catch {
      json = null;
    }
    return { ok: response.ok, status: response.status, body: json, text };
  } finally {
    clearTimeout(timeout);
  }
}

async function postStateful(runtime, endpointPath, repoRoot, payload, event, timeoutMs, sourceKind = 'ide') {
  return postJson(
    runtime,
    endpointPath,
    makeEnvelope(runtime, repoRoot, payload, event, sourceKind),
    timeoutMs,
  );
}

module.exports = {
  contentHash,
  makeEnvelope,
  postJson,
  postStateful,
  readRuntimeFile,
  saveCheckBody,
  renderConflictMessages,
  runtimeFilePath,
  sha256Hex,
};
