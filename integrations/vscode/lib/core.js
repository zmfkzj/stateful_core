'use strict';

const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');

function statefulHomePath(env = process.env, home = os.homedir()) {
  return env.STATEFUL_HOME || path.join(home, '.stateful_core');
}

function runtimeFilePath(env = process.env, home = os.homedir()) {
  return path.join(statefulHomePath(env, home), 'runtime', 'server.json');
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

function yamlBare(value) {
  const text = value.trim();
  const comment = text.search(/\s#/);
  return (comment < 0 ? text : text.slice(0, comment)).trim();
}

function yamlScalar(value) {
  const text = value.trim();
  if (!text) return null;
  if (text.startsWith('"') && text.endsWith('"')) {
    try {
      return JSON.parse(text);
    } catch {
      return null;
    }
  }
  if (text.startsWith("'") && text.endsWith("'")) return text.slice(1, -1).replace(/''/g, "'");
  return yamlBare(text);
}

function yamlBoolean(value) {
  const text = yamlBare(value);
  return text === 'true' || text === 'false' ? text : null;
}

function readRepoRegistry(filePath) {
  let lines;
  try {
    lines = fs.readFileSync(filePath, 'utf8').split(/\r?\n/);
  } catch {
    return [];
  }
  const entries = [];
  const required = ['repo_id', 'root', 'enabled', 'enabled_at', 'policy_config_path'];
  let inRepos = false;
  let entry;
  for (const line of lines) {
    if (!line.trim() || line.trimStart().startsWith('#')) continue;
    if (!inRepos) {
      if (/^repos:\s*(?:#.*)?$/.test(line)) inRepos = true;
      else if (/^repos:\s*\[\]\s*(?:#.*)?$/.test(line)) return [];
      continue;
    }
    const item = line.match(/^\s*-\s+([a-z_]+):\s*(.*?)\s*$/);
    if (item) {
      if (entry) entries.push(entry);
      entry = {};
      if (required.includes(item[1])) {
        const value = item[1] === 'enabled' ? yamlBoolean(item[2]) : yamlScalar(item[2]);
        if (value === null) return null;
        entry[item[1]] = value;
      }
      continue;
    }
    if (/^\S/.test(line)) break;
    const field = line.match(/^\s+([a-z_]+):\s*(.*?)\s*$/);
    if (!entry) return null;
    if (/^\s+-\s+/.test(line)) continue;
    if (!field) return null;
    if (!required.includes(field[1])) continue;
    if (Object.hasOwn(entry, field[1])) return null;
    const value = field[1] === 'enabled' ? yamlBoolean(field[2]) : yamlScalar(field[2]);
    if (value === null) return null;
    entry[field[1]] = value;
  }
  if (entry) entries.push(entry);
  return entries.every(
    (candidate) => required.every((field) => typeof candidate[field] === 'string' && candidate[field].length > 0)
      && ['true', 'false'].includes(candidate.enabled),
  ) ? entries : null;
}

function gitRoot(folder) {
  let current;
  try {
    current = fs.realpathSync(folder);
  } catch {
    return null;
  }
  if (fs.statSync(current).isFile()) current = path.dirname(current);
  for (;;) {
    if (fs.existsSync(path.join(current, '.git'))) return current;
    const parent = path.dirname(current);
    if (parent === current) return null;
    current = parent;
  }
}

function currentBranch(root) {
  const git = path.join(root, '.git');
  let head = path.join(git, 'HEAD');
  try {
    if (fs.statSync(git).isFile()) {
      const match = fs.readFileSync(git, 'utf8').match(/^gitdir:\s*(.+?)\s*$/);
      if (!match) return 'unknown';
      head = path.join(path.resolve(root, match[1]), 'HEAD');
    }
    const ref = fs.readFileSync(head, 'utf8').match(/^ref:\s*refs\/heads\/(.+?)\s*$/);
    return ref?.[1] || 'unknown';
  } catch {
    return 'unknown';
  }
}

function enabledRepoIdentity(folder, env = process.env, home = os.homedir()) {
  const root = gitRoot(folder);
  if (!root) return null;
  const entries = readRepoRegistry(path.join(statefulHomePath(env, home), 'config.yml'));
  if (!entries) return null;
  const entry = entries.find(
    (candidate) => candidate.enabled === 'true'
      && candidate.root === root
      && typeof candidate.repo_id === 'string'
      && candidate.repo_id !== 'unknown'
      && candidate.repo_id.length > 0,
  );
  if (!entry) return null;
  return {
    root,
    repoId: entry.repo_id,
    worktreeId: entry.repo_id,
    branch: currentBranch(root),
  };
}

function effectiveWorkspaceId(runtimeWorkspaceId, identity) {
  if (identity && ['local', 'shared', 'unknown'].includes(runtimeWorkspaceId)) {
    return `workspace-${identity.worktreeId.replace(/^repo-/, '')}`;
  }
  return runtimeWorkspaceId;
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


module.exports = {
  effectiveWorkspaceId,
  enabledRepoIdentity,
  contentHash,
  postJson,
  readRuntimeFile,
  renderConflictMessages,
  runtimeFilePath,
  sha256Hex,
};
