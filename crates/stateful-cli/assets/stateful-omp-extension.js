import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  closeSync,
  fsyncSync,
  linkSync,
  mkdirSync,
  openSync,
  readFileSync,
  renameSync,
  readdirSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const STATEFUL = __STATEFUL_BINARY_JSON__;
const OMP_VERSION = "17.2.3";
const HEARTBEAT_MS = 1_000;
const tasks = new Map();
const writeAttempts = new Map();
const pendingTerminals = new Map();
let heartbeatTimer;

function runStatefulHook(event, payload) {
  const result = spawnSync(STATEFUL, ["hook", "omp", event], {
    input: JSON.stringify(payload),
    encoding: "utf8",
  });
  if (result.status !== 0) {
    return { decision: "block", reason: String(result.stderr || "stateful hook failed").trim() };
  }
  try {
    return JSON.parse(String(result.stdout || "").trim() || '{"decision":"allow"}');
  } catch {
    return { decision: "block", reason: "stateful hook returned invalid JSON" };
  }
}

function requiredSessionValue(ctx, method, label) {
  const manager = ctx?.sessionManager;
  const value = manager && typeof manager[method] === "function" ? manager[method]() : undefined;
  if (typeof value !== "string" || !value.trim()) {
    throw new Error("Stateful requires OMP ctx.sessionManager." + method + "() for " + label);
  }
  return value.trim();
}

function owner(ctx) {
  const sessionId = requiredSessionValue(ctx, "getSessionId", "session ownership");
  const leafAgentId = requiredSessionValue(ctx, "getLeafId", "leaf-agent ownership");
  return { sessionId, leafAgentId, key: sessionId + ":" + leafAgentId };
}

function taskPayload(event, ctx, taskId, type) {
  const identity = owner(ctx);
  return {
    ...event,
    type,
    runtime: "omp",
    version: OMP_VERSION,
    cwd: ctx?.cwd,
    sessionId: identity.sessionId,
    leafAgentId: identity.leafAgentId,
    task_id: taskId,
  };
}

function terminalCorrelation(payload) {
  const correlation = [payload.attempt_id, payload.permit_id, payload.task_id, payload.toolCallId];
  if (
    !correlation.every(
      (value) => typeof value === "string" && value.length > 0 && value.length <= 4_096,
    )
  ) {
    throw new Error("Stateful cannot durably correlate an OMP terminal payload");
  }
  return createHash("sha256").update(JSON.stringify(correlation)).digest("hex");
}

function terminalOutboxDirectory() {
  const home = process.env.STATEFUL_HOME || join(process.env.HOME || homedir(), ".stateful_core");
  const directory = join(home, "omp-terminal-outbox");
  mkdirSync(directory, { recursive: true, mode: 0o700 });
  chmodSync(directory, 0o700);
  return directory;
}

function syncDirectory(directory) {
  const descriptor = openSync(directory, "r");
  try {
    fsyncSync(descriptor);
  } finally {
    closeSync(descriptor);
  }
}

function terminalRecord(payload) {
  const key = terminalCorrelation(payload);
  return { key, path: join(terminalOutboxDirectory(), key + ".json"), payload };
}

function persistTerminal(payload) {
  const record = terminalRecord(payload);
  const temporary = record.path + "." + process.pid + ".tmp";
  const descriptor = openSync(temporary, "w", 0o600);
  try {
    writeFileSync(descriptor, JSON.stringify(payload), "utf8");
    fsyncSync(descriptor);
  } finally {
    closeSync(descriptor);
  }
  chmodSync(temporary, 0o600);
  let created = false;
  try {
    linkSync(temporary, record.path);
    created = true;
  } catch (error) {
    if (error?.code !== "EEXIST") throw error;
    const existing = JSON.parse(readFileSync(record.path, "utf8"));
    if (terminalCorrelation(existing) !== record.key) throw error;
    record.payload = existing;
  } finally {
    try {
      unlinkSync(temporary);
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
  }
  if (created) syncDirectory(terminalOutboxDirectory());
  pendingTerminals.set(record.key, record);
  return record;
}

function loadPendingTerminals() {
  let directory;
  try {
    directory = terminalOutboxDirectory();
  } catch {
    return;
  }
  for (const name of readdirSync(directory)) {
    if (!/^[a-f0-9]{64}\.json$/.test(name)) continue;
    const path = join(directory, name);
    try {
      const payload = JSON.parse(readFileSync(path, "utf8"));
      const record = terminalRecord(payload);
      if (name !== record.key + ".json") {
        throw new Error("payload correlation does not match its filename");
      }
      pendingTerminals.set(record.key, record);
    } catch (error) {
      const quarantined = path + `.bad-${Date.now()}-${process.pid}`;
      renameSync(path, quarantined);
      syncDirectory(directory);
      console.error(
        `Stateful quarantined invalid OMP terminal outbox file ${path} as ${quarantined}: ${error}`,
      );
    }
  }
}

function removeTerminal(record) {
  try {
    unlinkSync(record.path);
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  syncDirectory(terminalOutboxDirectory());
  pendingTerminals.delete(record.key);
  stopHeartbeatWhenIdle();
}

function pendingTerminal(toolCallId) {
  for (const record of pendingTerminals.values()) {
    if (record.payload.toolCallId === toolCallId) return record;
  }
}

function retryPendingTerminals(matches = () => true) {
  for (const record of pendingTerminals.values()) {
    if (!matches(record.payload)) continue;
    try {
      if (runStatefulHook("post-tool-use", record.payload).decision === "allow") {
        removeTerminal(record);
      }
    } catch {
      // Preserve the exact terminal payload for the next lifecycle retry.
    }
  }
}

function startHeartbeat() {
  if (heartbeatTimer) return;
  heartbeatTimer = setInterval(() => {
    retryPendingTerminals();
    for (const task of tasks.values()) {
      runStatefulHook("post-tool-use", {
        type: "heartbeat",
        runtime: "omp",
        version: OMP_VERSION,
        cwd: task.cwd,
        sessionId: task.sessionId,
        leafAgentId: task.leafAgentId,
        task_id: task.taskId,
      });
    }
  }, HEARTBEAT_MS);
  heartbeatTimer.unref?.();
}

function stopHeartbeatWhenIdle() {
  if (tasks.size || pendingTerminals.size || !heartbeatTimer) return;
  clearInterval(heartbeatTimer);
  heartbeatTimer = undefined;
}

function taskFor(ctx) {
  const identity = owner(ctx);
  return tasks.get(identity.key);
}

function block(reason) {
  return { block: true, reason };
}

export default function statefulOmpExtension(pi) {
  pi.setLabel("Stateful");
  loadPendingTerminals();
  retryPendingTerminals();
  if (pendingTerminals.size) startHeartbeat();


  pi.on("agent_start", async (event, ctx) => {
    let identity;
    try {
      identity = owner(ctx);
    } catch (error) {
      return block(error instanceof Error ? error.message : String(error));
    }
    const result = runStatefulHook("session-start", taskPayload(event, ctx, undefined, "agent_start"));
    if (result.decision !== "allow" || typeof result.task_id !== "string" || !result.task_id) {
      return block(result.reason || "stateful task start failed");
    }
    tasks.set(identity.key, {
      taskId: result.task_id,
      sessionId: identity.sessionId,
      leafAgentId: identity.leafAgentId,
      cwd: ctx?.cwd,
    });
    startHeartbeat();
  });

  pi.on("agent_end", async (event, ctx) => {
    let identity;
    try {
      identity = owner(ctx);
    } catch {
      if (event?.willContinue !== true) retryPendingTerminals();
      return;
    }
    if (event?.willContinue === true) return;
    retryPendingTerminals(
      (payload) =>
        payload.sessionId === identity.sessionId && payload.leafAgentId === identity.leafAgentId,
    );
    const task = tasks.get(identity.key);
    if (!task) {
      stopHeartbeatWhenIdle();
      return;
    }
    runStatefulHook("stop", taskPayload(event, ctx, task.taskId, "agent_end"));
    tasks.delete(identity.key);
    stopHeartbeatWhenIdle();
  });

  pi.on("tool_call", async (event, ctx) => {
    let task;
    try {
      task = taskFor(ctx);
    } catch (error) {
      return block(error instanceof Error ? error.message : String(error));
    }
    if (!task) return block("Stateful has no active task for this OMP leaf agent");
    const result = runStatefulHook("pre-tool-use", taskPayload(event, ctx, task.taskId, "tool_call"));
    if (result.decision !== "allow") return block(result.reason || "stateful denied tool execution");
    const attempt = result.stateful?.write_attempt;
    if (attempt?.attempt_id && attempt?.permit_id && typeof event?.toolCallId === "string") {
      writeAttempts.set(event.toolCallId, {
        ...attempt,
        task_id: task.taskId,
        sessionId: task.sessionId,
        leafAgentId: task.leafAgentId,
        cwd: task.cwd,
      });
    }
  });

  pi.on("tool_result", async (event, ctx) => {
    const toolCallId = event?.toolCallId;
    const pending = pendingTerminal(toolCallId);
    if (pending) {
      retryPendingTerminals((payload) => payload === pending.payload);
      return;
    }
    const attempt = writeAttempts.get(toolCallId);
    let task;
    try {
      task = taskFor(ctx);
    } catch {
      task = undefined;
    }
    const identity = attempt || task;
    if (!identity) return;
    const payload = {
      ...event,
      type: "tool_result",
      runtime: "omp",
      version: OMP_VERSION,
      cwd: identity.cwd || ctx?.cwd,
      sessionId: identity.sessionId,
      leafAgentId: identity.leafAgentId,
      task_id: identity.task_id || identity.taskId,
    };
    if (!attempt) {
      runStatefulHook("post-tool-use", payload);
      return;
    }
    Object.assign(payload, attempt);
    const record = persistTerminal(payload);
    writeAttempts.delete(toolCallId);
    retryPendingTerminals((pendingPayload) => pendingPayload === record.payload);
  });

  pi.on("session_shutdown", async (event, ctx) => {
    const sessionId = typeof event?.sessionId === "string" ? event.sessionId : undefined;
    retryPendingTerminals((payload) => !sessionId || payload.sessionId === sessionId);
    for (const [key, task] of tasks) {
      if (sessionId && task.sessionId !== sessionId) continue;
      runStatefulHook("stop", {
        ...event,
        type: "session_shutdown",
        runtime: "omp",
        version: OMP_VERSION,
        cwd: task.cwd || ctx?.cwd,
        sessionId: task.sessionId,
        leafAgentId: task.leafAgentId,
        task_id: task.taskId,
      });
      tasks.delete(key);
    }
    stopHeartbeatWhenIdle();
  });
}
