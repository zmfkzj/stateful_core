// The Rust `stateful hook` pipeline is the enforcement authority. This
// extension's command parsing/preflight is advisory UX only; do not extend
// its quoting rules independently of crates/stateful-cli/src/shell_command.rs.
import { spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { delimiter, dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const STATEFUL = __STATEFUL_BINARY_JSON__;
const EXTENSION_DIR = dirname(fileURLToPath(import.meta.url));
const OMP_AGENT_CONFIG = resolve(EXTENSION_DIR, "..", "config.yml");
const BENCHMARK_SOURCE_BLOCK_ENV = "STATEFUL_BENCHMARK_SOURCE_BLOCK_PATTERNS";
 

function statefulBinaryDigest(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function executableFile(path) {
  try {
    const stat = statSync(path);
    return stat.isFile() && (stat.mode & 0o111) !== 0;
  } catch {
    return false;
  }
}

function firstPathStateful(cwd) {
  const base = cwd || process.cwd();
  for (const entry of String(process.env.PATH || "").split(delimiter)) {
    const directory = entry ? resolve(base, entry) : base;
    const candidate = resolve(directory, "stateful");
    if (executableFile(candidate)) return candidate;
  }
  return null;
}

function verifyBareStateful(cwd) {
  const candidate = firstPathStateful(cwd);
  if (!candidate) return false;
  try {
    if (statefulBinaryDigest(candidate) !== statefulBinaryDigest(STATEFUL)) return false;
    return true;
  } catch {
    return false;
  }
}


function isTrustedStatefulCommand(word, cwd) {
  if (word === STATEFUL) return true;
  if (word !== "stateful") return false;
  return verifyBareStateful(cwd);
}


function runStatefulHook(event, payload) {
  const result = spawnSync(STATEFUL, ["hook", "omp", event], {
    input: JSON.stringify(payload),
    encoding: "utf8",
  });
  if (result.status !== 0) {
    return { decision: "block", reason: result.stderr || "stateful hook failed" };
  }
  const text = (result.stdout || "").trim();
  return text ? JSON.parse(text) : { decision: "allow" };
}

function isYolo(event, ctx) {
  const values = [
    event?.yolo,
    event?.autoApprove,
    event?.approvalMode,
    ctx?.yolo,
    ctx?.autoApprove,
    ctx?.approvalMode,
    ctx?.config?.approvalMode,
    ctx?.config?.tools?.approvalMode,
  ];
  return values.some((value) => value === true || value === "yolo" || value === "auto-approve");
}

function firstString(...values) {
  for (const value of values) {
    if (typeof value === "string" && value.trim().length > 0) return value;
  }
  return undefined;
}

function agentIdFragmentFromString(value) {
  if (typeof value !== "string") return undefined;
  const id = value.trim();
  if (!id) return undefined;
  if (!/^[A-Za-z0-9_-]+$/.test(id)) return undefined;
  return id;
}

function sessionIdFromString(value) {
  if (typeof value !== "string") return undefined;
  const id = value.trim();
  if (!id) return undefined;
  if (!/^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}$/.test(id)) return undefined;
  return id;
}

function workspaceIdFromString(value) {
  if (typeof value !== "string") return undefined;
  const id = value.trim();
  return id.length > 0 ? id : undefined;
}

function sessionManagerString(ctx, method, parse) {
  const sessionManager = ctx?.sessionManager;
  if (!sessionManager || typeof sessionManager[method] !== "function") return undefined;
  try {
    return parse(sessionManager[method]());
  } catch {
    return undefined;
  }
}

function detectAgentId(_event, ctx) {
  const sessionId = sessionManagerString(ctx, "getSessionId", sessionIdFromString);
  if (!sessionId) return undefined;
  const leafId = sessionManagerString(ctx, "getLeafId", agentIdFragmentFromString);
  return leafId ? `omp-${sessionId}-${leafId}` : `omp-${sessionId}`;
}

function detectWorkspaceId(event, ctx) {
  return firstString(
    workspaceIdFromString(event?.workspaceId),
    workspaceIdFromString(event?.workspace_id),
    workspaceIdFromString(event?.workspace?.id),
    workspaceIdFromString(event?.workspace?.workspaceId),
    workspaceIdFromString(event?.workspace?.workspace_id),
    workspaceIdFromString(ctx?.workspaceId),
    workspaceIdFromString(ctx?.workspace_id),
    workspaceIdFromString(ctx?.workspace?.id),
    workspaceIdFromString(ctx?.workspace?.workspaceId),
    workspaceIdFromString(ctx?.workspace?.workspace_id)
  );
}

function missingAgentIdReason() {
  return "Stateful requires OMP ctx.sessionManager.getSessionId() to derive the active agent_id; no session id was available, so Stateful actions are disabled for this agent.";
}


function agentId(event, ctx) {
  const id = detectAgentId(event, ctx);
  if (!id) throw new Error(missingAgentIdReason());
  return id;
}

function reservationIdFromValue(value) {
  if (typeof value !== "string") return undefined;
  const id = value.trim();
  return id.length > 0 ? id : undefined;
}

function reservationId(event, decision) {
  return firstString(
    reservationIdFromValue(event?.reservation_id),
    reservationIdFromValue(event?.reservationId),
    reservationIdFromValue(event?.input?.reservation_id),
    reservationIdFromValue(event?.input?.reservationId),
    reservationIdFromValue(decision?.reservation_id),
    reservationIdFromValue(decision?.wait?.reservation_id),
    reservationIdFromValue(decision?.reservation?.reservation_id),
    reservationIdFromValue(decision?.reservation?.wait_id),
    reservationIdFromValue(decision?.reservation?.id)
  );
}

let contextStreamAbort;
let contextStreamKey = "";
let contextStreamLastEventId = "";
let activeContextStream;
let contextState = {
  deliveredVersion: undefined,
  pendingVersion: undefined,
  initialPending: false,
  deliveryInFlight: false,
};

function version(value) {
  return Number.isSafeInteger(value) && value >= 0 ? value : undefined;
}

export function exactReadCandidate(event) {
  const toolName = event?.toolName;
  if (toolName !== "read" && toolName !== "functions.read") return false;
  const path = event?.input?.path;
  if (typeof path !== "string" || !path.endsWith(":raw")) return false;
  const source = path.slice(0, -":raw".length);
  if (!source || /:\d+(?:[-+]\d*)?(?:,\d+(?:[-+]\d*)?)*$/.test(source)) return false;
  const metadata = event?.resultMetadata || event?.result_metadata || event?.result || {};
  return event?.isError !== true
    && event?.isComplete !== false
    && event?.complete !== false
    && event?.truncated !== true
    && event?.isTruncated !== true
    && metadata?.truncated !== true;
}

export function coalesceContextInvalidation(currentVersion, targetVersion) {
  const current = version(currentVersion);
  const target = version(targetVersion);
  if (target === undefined) return current;
  return current === undefined || target > current ? target : current;
}

export function shouldDeliverContextVersion(deliveredVersion, targetVersion) {
  const target = version(targetVersion);
  if (target === undefined) return false;
  const delivered = version(deliveredVersion);
  return delivered === undefined || target > delivered;
}

function stopContextStream() {
  if (contextStreamAbort) {
    contextStreamAbort.abort();
    contextStreamAbort = undefined;
  }
  activeContextStream = undefined;
}

function sleepWithAbort(ms, signal) {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    if (signal) {
      signal.addEventListener("abort", () => {
        clearTimeout(timer);
        resolve();
      }, { once: true });
    }
  });
}

function contextIdentity(stream) {
  const streamAgent = stream?.agent || {};
  const streamWorkspace = stream?.workspace || {};
  const agentId = firstString(streamAgent.agent_id, stream?.agent_id);
  const workspaceId = firstString(streamWorkspace.workspace_id, stream?.workspace_id);
  return {
    agent: {
      agent_id: agentId,
      actor_id: firstString(streamAgent.actor_id, stream?.actor_id, agentId),
      actor_type: firstString(streamAgent.actor_type, stream?.actor_type, "agent"),
    },
    workspace: {
      root: firstString(streamWorkspace.root, stream?.root, stream?.cwd, "."),
      workspace_id: workspaceId,
      repo_id: firstString(streamWorkspace.repo_id, stream?.repo_id, "unknown"),
      worktree_id: firstString(streamWorkspace.worktree_id, stream?.worktree_id, "unknown"),
      branch: firstString(streamWorkspace.branch, stream?.branch, "unknown"),
    },
  };
}

function contextEnvelope(stream, event, payload, toolName) {
  const identity = contextIdentity(stream);
  const source = {
    kind: "hook",
    event,
    source_ref: "omp:" + event,
  };
  if (toolName) source.tool_name = toolName;
  return {
    protocol_version: "stateful.v2",
    request_id: randomUUID(),
    observed_at: new Date().toISOString(),
    agent: identity.agent,
    workspace: identity.workspace,
    source,
    payload,
  };
}

async function postContextV2(stream, path, event, payload, toolName) {
  if (typeof fetch !== "function") return undefined;
  try {
    return await fetch(String(stream.base_url || "").replace(/\/+$/, "") + path, {
      method: "POST",
      headers: {
        authorization: stream.authorization,
        "content-type": "application/json",
      },
      body: JSON.stringify(contextEnvelope(stream, event, payload, toolName)),
    });
  } catch (_) {
    return undefined;
  }
}

function contextStreamUrl(stream) {
  const envelope = contextEnvelope(stream, "omp_notification_stream", {});
  const query = new URLSearchParams({
    protocol_version: envelope.protocol_version,
    request_id: envelope.request_id,
    observed_at: envelope.observed_at,
    agent_id: envelope.agent.agent_id,
    actor_id: envelope.agent.actor_id,
    actor_type: envelope.agent.actor_type,
    root: envelope.workspace.root,
    workspace_id: envelope.workspace.workspace_id,
    repo_id: envelope.workspace.repo_id,
    worktree_id: envelope.workspace.worktree_id,
    branch: envelope.workspace.branch,
    kind: envelope.source.kind,
    event: envelope.source.event,
    source_ref: envelope.source.source_ref,
  });
  return String(stream.base_url || "").replace(/\/+$/, "") + "/v2/notifications/stream?" + query;
}

function notificationTargetsStreamAgent(notification, stream) {
  const targetAgentId = notification?.target_agent_id
    || notification?.agent_id
    || notification?.payload?.agent_id;
  if (!targetAgentId) return true;
  return targetAgentId === stream?.agent_id;
}

function reservationMessage(notification) {
  const payload = notification?.payload || {};
  const target = payload.relative_path || "the reserved target";
  const waitId = payload.wait_id || "unknown";
  const reservationId = payload.reservation_id || waitId;
  const action = payload.action || "write";
  const purpose = payload.purpose;
  const lines = [
    "Stateful reservation is ready for " + target + ".",
    "wait_id: " + waitId,
    "reservation_id: " + reservationId,
    "action: " + action,
  ];
  if (typeof purpose === "string" && purpose.trim().length > 0) {
    lines.push("purpose: " + purpose.trim());
  }
  lines.push("Next: reread the target, then resume the saved lazy operation or retry the write so the write boundary can claim the reservation. Only clients with an exposed state_reservation_claim tool should claim manually first.");
  return lines.join("\n");
}

function deliverReservationNotification(pi, notification, stream) {
  if (!notificationTargetsStreamAgent(notification, stream)) return true;
  bindGrantedLazyReservation(notification);
  if (typeof pi?.sendMessage !== "function") return false;
  try {
    pi.sendMessage(
      {
        customType: "stateful_reservation_ready",
        content: reservationMessage(notification),
        display: true,
        details: notification,
      },
      { triggerTurn: true, deliverAs: "nextTurn" }
    );
    return true;
  } catch (_) {
    return false;
  }
}

function deliverCoordinationWarning(pi, notification) {
  if (typeof pi?.sendMessage !== "function") return false;
  const payload = notification?.payload || {};
  const content = firstString(payload.message, notification?.message, "Stateful detected overlapping work.");
  try {
    pi.sendMessage(
      { customType: "stateful_coordination_warning", content, display: true },
      { deliverAs: "nextTurn" }
    );
    return true;
  } catch (_) {
    return false;
  }
}

async function acknowledgeNotification(stream, notification) {
  const sequence = version(notification?.sequence);
  if (sequence === undefined) return true;
  const response = await postContextV2(stream, "/v2/notifications/poll", "omp_notification_ack", { sequence });
  return response?.ok === true;
}

async function deliverContext(pi, stream, targetVersion) {
  if (targetVersion !== undefined && !shouldDeliverContextVersion(contextState.deliveredVersion, targetVersion)) {
    return true;
  }
  const response = await postContextV2(stream, "/v2/context/render", "omp_context_render", { mode: "brief" });
  if (!response?.ok) return false;
  let context;
  try {
    context = await response.json();
  } catch (_) {
    return false;
  }
  const renderedVersion = version(context?.workspace_version);
  if (renderedVersion === undefined) return false;
  if (targetVersion !== undefined && renderedVersion < targetVersion) return false;
  if (!context?.changed) {
    contextState.deliveredVersion = coalesceContextInvalidation(contextState.deliveredVersion, renderedVersion);
    return true;
  }
  if (!shouldDeliverContextVersion(contextState.deliveredVersion, renderedVersion)) return true;
  if (!context?.delivery_id || version(context?.sequence) === undefined || typeof pi?.sendMessage !== "function") {
    return false;
  }
  try {
    pi.sendMessage(
      {
        customType: "stateful_context",
        content: String(context.prompt_text || ""),
        display: true,
      },
      { triggerTurn: true, deliverAs: "nextTurn" }
    );
  } catch (_) {
    return false;
  }
  const acknowledgement = await postContextV2(stream, "/v2/context/ack", "context_ack", {
    delivery_id: context.delivery_id,
    sequence: context.sequence,
    workspace_version: renderedVersion,
  });
  if (!acknowledgement?.ok) return false;
  contextState.deliveredVersion = coalesceContextInvalidation(contextState.deliveredVersion, renderedVersion);
  return true;
}

function queueContextInvalidation(targetVersion) {
  const target = version(targetVersion);
  if (target === undefined) return false;
  contextState.pendingVersion = coalesceContextInvalidation(contextState.pendingVersion, target);
  return true;
}

async function flushContextDelivery(pi, stream) {
  if (contextState.deliveryInFlight) return true;
  contextState.deliveryInFlight = true;
  try {
    if (contextState.initialPending) {
      contextState.initialPending = false;
      if (!await deliverContext(pi, stream)) {
        contextState.initialPending = true;
        return false;
      }
    }
    const target = contextState.pendingVersion;
    if (target === undefined) return true;
    contextState.pendingVersion = undefined;
    if (!shouldDeliverContextVersion(contextState.deliveredVersion, target)) return true;
    if (await deliverContext(pi, stream, target)) return true;
    contextState.pendingVersion = coalesceContextInvalidation(contextState.pendingVersion, target);
    return false;
  } finally {
    contextState.deliveryInFlight = false;
  }
}

async function processContextNotification(pi, notification, stream, event) {
  if (!notificationTargetsStreamAgent(notification, stream)) return true;
  const kind = event === "message" || event === "notification" ? notification?.kind : event;
  if (kind === "context_invalidated") {
    if (!queueContextInvalidation(notification?.payload?.target_version)) return false;
    return flushContextDelivery(pi, stream);
  }
  if (kind === "reservation_granted") {
    return deliverReservationNotification(pi, notification, stream);
  }
  if (kind === "scope_overlap") {
    return deliverCoordinationWarning(pi, notification);
  }
  return true;
}

async function recoverContextNotifications(pi, stream) {
  const response = await postContextV2(stream, "/v2/notifications/poll", "omp_notification_poll", {});
  if (!response?.ok) return false;
  let notifications;
  try {
    notifications = await response.json();
  } catch (_) {
    return false;
  }
  if (!Array.isArray(notifications)) return false;
  for (const notification of notifications) {
    if (!await processContextNotification(pi, notification, stream, notification?.kind || "message")) return false;
    if (!await acknowledgeNotification(stream, notification)) return false;
  }
  return flushContextDelivery(pi, stream);
}

async function processContextSseBlock(pi, block, stream) {
  let event = "message";
  let id = "";
  const data = [];
  for (const rawLine of block.split(/\r?\n/)) {
    const line = rawLine.trimEnd();
    if (line.startsWith("id:")) id = line.slice(3).trim();
    if (line.startsWith("event:")) event = line.slice(6).trim();
    if (line.startsWith("data:")) data.push(line.slice(5).trimStart());
  }
  if (data.length === 0) return;
  try {
    const notification = JSON.parse(data.join("\n"));
    if (await processContextNotification(pi, notification, stream, event)) {
      if (!await acknowledgeNotification(stream, notification)) return;
      if (id) contextStreamLastEventId = id;
    }
  } catch (_) {}
}

async function processContextSseBuffer(pi, buffer, stream) {
  buffer = buffer.replace(/\r\n/g, "\n");
  let cursor = 0;
  for (;;) {
    const next = buffer.indexOf("\n\n", cursor);
    if (next === -1) break;
    await processContextSseBlock(pi, buffer.slice(cursor, next), stream);
    cursor = next + 2;
  }
  return buffer.slice(cursor);
}

function activateContextStream(stream, reset = false) {
  const streamKey = stream.agent_id + "\u0000" + stream.workspace_id;
  if (reset || contextStreamKey !== streamKey) {
    contextStreamKey = streamKey;
    contextStreamLastEventId = "";
    contextState = {
      deliveredVersion: undefined,
      pendingVersion: undefined,
      initialPending: false,
      deliveryInFlight: false,
    };
  }
  activeContextStream = stream;
}

function startContextStream(pi, stream) {
  if (!stream?.base_url || !stream?.authorization || !stream?.agent_id || !stream?.workspace_id) return;
  if (typeof fetch !== "function" || typeof TextDecoder !== "function") return;
  stopContextStream();
  activateContextStream(stream);
  const controller = new AbortController();
  contextStreamAbort = controller;
  const signal = controller.signal;
  const run = async () => {
    let backoffMs = 1000;
    while (!signal.aborted) {
      try {
        const headers = { authorization: stream.authorization, accept: "text/event-stream" };
        if (contextStreamLastEventId) headers["last-event-id"] = contextStreamLastEventId;
        const response = await fetch(contextStreamUrl(stream), { headers, signal });
        if (!response.ok || !response.body?.getReader) throw new Error("context stream unavailable");
        backoffMs = 1000;
        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";
        for (;;) {
          const { done, value } = await reader.read();
          if (done || signal.aborted) break;
          buffer = await processContextSseBuffer(pi, buffer + decoder.decode(value, { stream: true }), stream);
        }
      } catch (_) {
        if (signal.aborted) return;
        await recoverContextNotifications(pi, stream);
        await sleepWithAbort(backoffMs, signal);
        backoffMs = Math.min(backoffMs * 2, 30000);
      }
    }
  };
  run().catch(() => {});
}


const EXTERNAL_GRANT_DEFAULT_MAX_USES = 5;
const EXTERNAL_GRANT_MAX_USES_LIMIT = 20;
const EXTERNAL_GRANT_DEFAULT_TTL_MS = 10 * 60 * 1000;
const EXTERNAL_GRANT_MAX_TTL_MS = 60 * 60 * 1000;
const externalBashGrants = new Map();


function stringList(value) {
  if (Array.isArray(value)) {
    return value.filter((item) => typeof item === "string" && item.trim().length > 0);
  }
  if (typeof value === "string" && value.trim().length > 0) {
    return [value];
  }
  return [];
}


const lazyEditOperations = new Map();
let lazyEditOperationCounter = 0;
const lazyWriteOperations = new Map();
let lazyWriteOperationCounter = 0;
const lazyBashOperations = new Map();
let lazyBashOperationCounter = 0;

export function bindGrantedLazyReservation(notification) {
  const payload = notification?.payload || {};
  const waitId = String(payload.wait_id || "").trim();
  const reservationId = String(payload.reservation_id || "").trim();
  if (!waitId || !reservationId) return false;
  const grantedPath = payload.relative_path === undefined ? "" : String(payload.relative_path).trim();
  let bound = false;
  for (const operations of [lazyEditOperations, lazyWriteOperations]) {
    const operation = operations.get(waitId);
    if (!operation) continue;
    if (grantedPath && (!safeLazyOperationTarget(grantedPath) || grantedPath !== operation.claim_path)) {
      continue;
    }
    operation.reservation_id = reservationId;
    operation.claimable = true;
    bound = true;
  }
  return bound;
}

function extractWaitId(reason) {
  const match = String(reason || "").match(/wait_id ([A-Za-z0-9_-]+)/);
  return match ? match[1] : "";
}

function extractReservationId(reason) {
  const match = String(reason || "").match(/reservation_id[: ]+([A-Za-z0-9_-]+)/);
  return match ? match[1] : "";
}

function structuredLazyWaitId(decision) {
  return decision?.wait?.wait_id
    || decision?.reservation?.wait_id
    || decision?.reservation?.id
    || extractWaitId(decision?.reason)
    || extractWaitId(decision?.message);
}

function structuredLazyEditOperationId(decision) {
  return structuredLazyWaitId(decision);
}

function structuredLazyWriteOperationId(decision) {
  return structuredLazyWaitId(decision);
}

function structuredLazyReservationId(event, decision) {
  return reservationId(event, decision)
    || extractReservationId(decision?.reason)
    || extractReservationId(decision?.message)
    || "";
}

function nextLazyEditOperationId() {
  lazyEditOperationCounter += 1;
  return "lazy-edit-" + Date.now().toString(36) + "-" + lazyEditOperationCounter.toString(36);
}

function nextLazyWriteOperationId() {
  lazyWriteOperationCounter += 1;
  return "lazy-write-" + Date.now().toString(36) + "-" + lazyWriteOperationCounter.toString(36);
}

function nextLazyBashOperationId() {
  lazyBashOperationCounter += 1;
  return "lazy-bash-" + Date.now().toString(36) + "-" + lazyBashOperationCounter.toString(36);
}

function editPatchTargets(input) {
  const patch = String(input?.input || "");
  const targets = [];
  for (const line of patch.split(/\r?\n/)) {
    const match = line.match(/^\[([^#\]\r\n]+)#[0-9A-Fa-f]{4}\]$/);
    if (match) targets.push(match[1]);
  }
  return [...new Set(targets)];
}

function safeLazyOperationTarget(target) {
  return typeof target === "string"
    && target.length > 0
    && !target.startsWith("/")
    && !target.includes("\\")
    && !target.includes(":")
    && !target.split("/").some((part) => part === "" || part === "." || part === "..");
}

function repoRelativeLazyTarget(cwd, target) {
  if (!safeLazyOperationTarget(target)) return "";
  const base = resolve(cwd || process.cwd());
  let root = base;
  while (!existsSync(resolve(root, ".git"))) {
    const parent = dirname(root);
    if (parent === root) return "";
    root = parent;
  }
  const normalized = relative(root, resolve(base, target)).replace(/\\/g, "/");
  return safeLazyOperationTarget(normalized) ? normalized : "";
}

function readOperationBase(path) {
  if (!existsSync(path)) return { ok: true, value: null };
  try {
    return { ok: true, value: readFileSync(path, "utf8") };
  } catch (error) {
    return { ok: false, error: error?.message || String(error) };
  }
}

function readOperationBases(cwd, targets) {
  const bases = new Map();
  for (const target of targets) {
    const path = resolve(cwd, target);
    const base = readOperationBase(path);
    if (!base.ok) return null;
    bases.set(target, base.value);
  }
  return bases;
}

function rememberLazyEditOperation(event, ctx, decision) {
  if (event?.toolName !== "edit") return "";
  const targets = editPatchTargets(event.input || {});
  if (targets.length === 0 || !targets.every(safeLazyOperationTarget)) return "";
  const bases = readOperationBases(ctx.cwd, targets);
  if (!bases) return "";
  const toolCallId = String(event?.toolCallId || "").trim();
  if (!toolCallId) return "";
  const waitId = structuredLazyWaitId(decision);
  const claimPath = targets.length === 1 ? repoRelativeLazyTarget(ctx.cwd, targets[0]) : "";
  if (waitId && !claimPath) return "";
  const operationId = waitId || nextLazyEditOperationId();
  lazyEditOperations.set(operationId, {
    tool_call_id: toolCallId,
    agent_id: agentId(event, ctx),
    workspace_id: detectWorkspaceId(event, ctx),
    wait_id: waitId,
    reservation_id: structuredLazyReservationId(event, decision),
    claim_path: claimPath,
    cwd: ctx.cwd,
    tool_name: event.toolName,
    tool_input: event.input || {},
    targets,
    bases,
    blocked_reason: decision?.reason || "",
  });
  return operationId;
}

function writeToolTarget(input) {
  const target = String(input?.path || "").trim();
  return safeLazyOperationTarget(target) ? target : "";
}

function rememberLazyWriteOperation(event, ctx, decision) {
  if (event?.toolName !== "write") return "";
  const target = writeToolTarget(event.input || {});
  if (!target) return "";
  const targets = [target];
  const bases = readOperationBases(ctx.cwd, targets);
  if (!bases) return "";
  const toolCallId = String(event?.toolCallId || "").trim();
  if (!toolCallId) return "";
  const waitId = structuredLazyWaitId(decision);
  const claimPath = repoRelativeLazyTarget(ctx.cwd, target);
  if (waitId && !claimPath) return "";
  const operationId = waitId || nextLazyWriteOperationId();
  lazyWriteOperations.set(operationId, {
    tool_call_id: toolCallId,
    agent_id: agentId(event, ctx),
    workspace_id: detectWorkspaceId(event, ctx),
    wait_id: waitId,
    reservation_id: structuredLazyReservationId(event, decision),
    claim_path: claimPath,
    cwd: ctx.cwd,
    tool_name: event.toolName,
    tool_input: event.input || {},
    targets,
    bases,
    blocked_reason: decision?.reason || "",
  });
  return operationId;
}

function normalizedStatefulCommandWords(words) {
  const normalized = [...words];
  if (normalized[0] === "stateful") normalized[0] = STATEFUL;
  return normalized;
}

function rememberLazyBashOperation(event, ctx, decision) {
  if (event?.toolName !== "bash" && event?.toolName !== "functions.bash") return "";
  if (!decision?.externalGrantParams || !Array.isArray(decision?.words)) return "";
  const operationId = nextLazyBashOperationId();
  lazyBashOperations.set(operationId, {
    operation_id: operationId,
    agent_id: agentId(event, ctx),
    cwd: ctx.cwd,
    tool_name: event.toolName,
    tool_input: event.input || {},
    command: String(event?.input?.command || ""),
    command_words: normalizedStatefulCommandWords(decision.words),
    grant_params: decision.externalGrantParams,
  });
  return operationId;
}

function textToLines(text) {
  if (text === "") return { lines: [], trailing: false };
  const trailing = text.endsWith("\n");
  const body = trailing ? text.slice(0, -1) : text;
  return { lines: body.length ? body.split("\n") : [], trailing };
}

function linesToText(lines, trailing) {
  return lines.join("\n") + (trailing && lines.length ? "\n" : "");
}

function readPatchBody(lines, cursor) {
  const body = [];
  while (cursor < lines.length && lines[cursor].startsWith("+")) {
    body.push(lines[cursor].slice(1));
    cursor += 1;
  }
  return { body, cursor };
}

function parseOmpLinePatch(patch) {
  const lines = String(patch || "").replace(/\r\n/g, "\n").split("\n");
  const files = new Map();
  let current = null;
  for (let i = 0; i < lines.length;) {
    const line = lines[i];
    if (line === "*** Begin Patch" || line === "*** End Patch") { i += 1; continue; }
    if (!line) { i += 1; continue; }
    const header = line.match(/^\[([^#\]\r\n]+)#[0-9A-Fa-f]{4}\]$/);
    if (header) {
      current = header[1];
      if (!files.has(current)) files.set(current, []);
      i += 1;
      continue;
    }
    if (!current) throw new Error("lazy_edit_resume patch missing file header");
    if (/^(SWAP|DEL)\.BLK |^INS\.BLK\.POST /.test(line)) {
      throw new Error("lazy_edit_resume supports line edits only; regenerate patch for block operations");
    }
    let match = line.match(/^SWAP ([1-9]\d*)\.=([1-9]\d*):$/);
    if (match) {
      const read = readPatchBody(lines, i + 1);
      files.get(current).push({ kind: "swap", start: Number(match[1]), end: Number(match[2]), body: read.body });
      i = read.cursor;
      continue;
    }
    match = line.match(/^DEL ([1-9]\d*)(?:\.=([1-9]\d*))?$/);
    if (match) {
      files.get(current).push({ kind: "del", start: Number(match[1]), end: Number(match[2] || match[1]), body: [] });
      i += 1;
      continue;
    }
    match = line.match(/^INS\.(HEAD|TAIL):$/);
    if (match) {
      const read = readPatchBody(lines, i + 1);
      files.get(current).push({ kind: "ins", pos: match[1].toLowerCase(), line: 0, body: read.body });
      i = read.cursor;
      continue;
    }
    match = line.match(/^INS\.(PRE|POST) ([1-9]\d*):$/);
    if (match) {
      const read = readPatchBody(lines, i + 1);
      files.get(current).push({ kind: "ins", pos: match[1].toLowerCase(), line: Number(match[2]), body: read.body });
      i = read.cursor;
      continue;
    }
    throw new Error("unsupported lazy_edit_resume patch line: " + line);
  }
  return files;
}

function validateOmpLinePatchBases(cwd, editsByFile, bases) {
  for (const target of editsByFile.keys()) {
    const filePath = resolve(cwd, target);
    const current = readOperationBase(filePath);
    if (!current.ok) return { status: "stale", message: target + " cannot be read for stale check" };
    if (current.value !== (bases.get(target) ?? null)) {
      return { status: "stale", message: target + " changed since operation was queued" };
    }
  }
  return null;
}

function applyOmpLinePatch(cwd, patch, bases) {
  const editsByFile = parseOmpLinePatch(patch);
  const stale = validateOmpLinePatchBases(cwd, editsByFile, bases);
  if (stale) return stale;
  for (const [target, edits] of editsByFile.entries()) {
    const filePath = resolve(cwd, target);
    const current = readOperationBase(filePath);
    if (!current.ok) return { status: "stale", message: target + " cannot be read for patch application" };
    const text = current.value || "";
    const split = textToLines(text);
    const applied = split.lines.slice();
    const ordered = edits.slice().sort((a, b) => {
      const aLine = a.kind === "ins" ? (a.pos === "tail" ? Number.MAX_SAFE_INTEGER : a.line) : a.start;
      const bLine = b.kind === "ins" ? (b.pos === "tail" ? Number.MAX_SAFE_INTEGER : b.line) : b.start;
      return bLine - aLine;
    });
    for (const edit of ordered) {
      if (edit.kind === "swap") {
        if (edit.start < 1 || edit.end < edit.start || edit.end > applied.length) throw new Error("invalid SWAP range for " + target);
        applied.splice(edit.start - 1, edit.end - edit.start + 1, ...edit.body);
      } else if (edit.kind === "del") {
        if (edit.start < 1 || edit.end < edit.start || edit.end > applied.length) throw new Error("invalid DEL range for " + target);
        applied.splice(edit.start - 1, edit.end - edit.start + 1);
      } else if (edit.kind === "ins") {
        const index = edit.pos === "head" ? 0 : edit.pos === "tail" ? applied.length : edit.pos === "pre" ? edit.line - 1 : edit.line;
        if (index < 0 || index > applied.length) throw new Error("invalid INS anchor for " + target);
        applied.splice(index, 0, ...edit.body);
      }
    }
    writeFileSync(filePath, linesToText(applied, split.trailing || text === ""), "utf8");
  }
  return { status: "applied", message: "lazy edit applied" };
}

function applyOmpWrite(cwd, operation) {
  const target = operation.targets[0];
  const stale = validateOmpLinePatchBases(cwd, new Map([[target, []]]), operation.bases);
  if (stale) return stale;
  const filePath = resolve(cwd, target);
  mkdirSync(dirname(filePath), { recursive: true });
  writeFileSync(filePath, String(operation.tool_input?.content ?? ""), "utf8");
  return { status: "applied", message: "lazy write applied" };
}

function emptyToolOutputText(text) {
  if (!String(text || "").trim()) return "No output.";
  return String(text);
}

function lazyToolResult(status, text, details) {
  return {
    isError: status !== "applied",
    content: [{ type: "text", text: emptyToolOutputText(text) }],
    details,
  };
}

function bashResumeText(result) {
  if (result.error) return result.error.message || String(result.error);
  const stdout = String(result.stdout || "");
  const stderr = String(result.stderr || "");
  const text = stdout + stderr;
  return text.trim().length ? text : "Command exited with code " + result.status;
}




function hasExternalWriteScope(params) {
  return stringList(params.write_targets).length > 0
    || stringList(params.create_targets).length > 0
    || stringList(params.write_dirs).length > 0;
}


function normalizedStringList(value) {
  return stringList(value).map((item) => item.trim()).sort();
}

function externalGrantSettings(params) {
  const requestedMaxUses = params.grant_max_uses ?? params.grantMaxUses;
  const requestedTtlSeconds = params.grant_expires_seconds ?? params.grantExpiresSeconds;
  let maxUses = EXTERNAL_GRANT_DEFAULT_MAX_USES;
  if (requestedMaxUses !== undefined) {
    if (!Number.isInteger(requestedMaxUses) || requestedMaxUses < 1 || requestedMaxUses > EXTERNAL_GRANT_MAX_USES_LIMIT) {
      throw new Error("external sandbox grant grant_max_uses must be an integer from 1 to " + EXTERNAL_GRANT_MAX_USES_LIMIT);
    }
    maxUses = requestedMaxUses;
  }
  let ttlMs = EXTERNAL_GRANT_DEFAULT_TTL_MS;
  if (requestedTtlSeconds !== undefined) {
    if (!Number.isInteger(requestedTtlSeconds) || requestedTtlSeconds < 1 || requestedTtlSeconds > EXTERNAL_GRANT_MAX_TTL_MS / 1000) {
      throw new Error("external sandbox grant grant_expires_seconds must be an integer from 1 to " + (EXTERNAL_GRANT_MAX_TTL_MS / 1000));
    }
    ttlMs = requestedTtlSeconds * 1000;
  }
  return { maxUses, ttlMs };
}

function externalGrantDescriptor(params) {
  return {
    purpose: params.purpose.trim(),
    write_targets: normalizedStringList(params.write_targets),
    create_targets: normalizedStringList(params.create_targets),
    write_dirs: normalizedStringList(params.write_dirs),
    connect_sockets: normalizedStringList(params.connect_sockets),
    allow_signal: params.allow_signal === true,
    network: typeof params.network === "string" ? params.network : "default",
  };
}


function externalGrantKey(params) {
  return JSON.stringify(externalGrantDescriptor(params));
}

function pruneExternalBashGrants(now) {
  for (const [key, grant] of externalBashGrants) {
    if (grant.expiresAt <= now || grant.uses >= grant.maxUses) {
      externalBashGrants.delete(key);
    }
  }
}

function configBool(value) {
  return value === true || value === "true" || value === "1" || value === "yes" || value === "on";
}

function benchmarkSourceBlockPatterns() {
  const raw = process.env[BENCHMARK_SOURCE_BLOCK_ENV];
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed)) {
      return parsed.map((item) => String(item || "").trim()).filter(Boolean);
    }
  } catch (_) {}
  return String(raw).split(/[\r\n,]+/).map((item) => item.trim()).filter(Boolean);
}

function benchmarkSourcePatternMatches(text, pattern) {
  const lowerPattern = pattern.toLowerCase();
  if (lowerPattern === "upstream" || lowerPattern === "upstream/") {
    return /(^|[^a-z0-9_-])upstream(?:\/|[^a-z0-9_-]|$)/.test(text);
  }
  return text.includes(lowerPattern);
}


function benchmarkSourceBlockReason(event) {
  const patterns = benchmarkSourceBlockPatterns();
  if (patterns.length === 0) return "";
  const text = (String(event?.toolName || "") + "\n" + JSON.stringify(event?.input || {})).toLowerCase();
  for (const pattern of patterns) {
    if (benchmarkSourcePatternMatches(text, pattern)) {
      return "DeNovo benchmark blocked target upstream source access before tool execution: " + pattern;
    }
  }
  return "";
}

function configTextAutoApprove(text) {
  const value = "(?:true|\\\"true\\\"|'true'|1|\\\"1\\\"|'1'|yes|\\\"yes\\\"|'yes'|on|\\\"on\\\"|'on')";
  const body = String(text || "");
  return new RegExp("(^|\\n)\\s*stateful\\.autoApprove\\s*:\\s*" + value + "\\s*(?:#.*)?(?:\\n|$)", "i").test(body)
    || new RegExp("(^|\\n)stateful\\s*:\\s*\\n(?:[ \\t]+[^\\n]*\\n)*?[ \\t]+autoApprove\\s*:\\s*" + value + "\\s*(?:#.*)?(?:\\n|$)", "i").test(body);
}

function statefulConfigFileAutoApprove() {
  const configPaths = [
    OMP_AGENT_CONFIG,
    process.env.HOME ? resolve(process.env.HOME, ".omp/profiles/stateful/agent/config.yml") : "",
  ].filter(Boolean);
  for (const configPath of configPaths) {
    try {
      if (configTextAutoApprove(readFileSync(configPath, "utf8"))) return true;
    } catch (_) {}
  }
  return false;
}

function statefulPromptAutoApproveConfig(ctx) {
  return configBool(ctx?.config?.stateful?.autoApprove)
    || configBool(ctx?.config?.["stateful.autoApprove"])
    || configBool(ctx?.stateful?.autoApprove)
    || statefulConfigFileAutoApprove();
}

function shouldAutoApproveStatefulPrompt(ctx, _params) {
  return statefulPromptAutoApproveConfig(ctx);
}

function recordExternalBashGrant(params, now) {
  const key = externalGrantKey(params);
  const settings = externalGrantSettings(params);
  const approvedAt = now ?? Date.now();
  externalBashGrants.set(key, {
    expiresAt: approvedAt + settings.ttlMs,
    maxUses: settings.maxUses,
    uses: 1,
  });
}

function approveExternalBashGrantWithoutPrompt(params) {
  const now = Date.now();
  pruneExternalBashGrants(now);
  const key = externalGrantKey(params);
  const existing = externalBashGrants.get(key);
  if (existing && existing.expiresAt > now && existing.uses < existing.maxUses) {
    existing.uses += 1;
    return true;
  }
  recordExternalBashGrant(params, now);
  return true;
}

function externalBashApprovalMessage(params) {
  const descriptor = externalGrantDescriptor(params);
  const settings = externalGrantSettings(params);
  const scope = [
    ...descriptor.write_targets.map((path) => "write-target: " + path),
    ...descriptor.create_targets.map((path) => "create-target: " + path),
    ...descriptor.write_dirs.map((path) => "write-dir: " + path),
    ...descriptor.connect_sockets.map((path) => "connect-socket: " + path),
    ...(descriptor.allow_signal ? ["allow-signal"] : []),
    "network: " + descriptor.network,
  ];
  const examples = stringList(params.approval_examples);
  return [
    "Stateful is requesting a scoped repo-external sandbox grant.",
    "",
    "Purpose:",
    descriptor.purpose,
    "",
    "Allowed external write/socket/signal scope:",
    scope.length ? scope.join("\n") : "No declared external write/socket/signal scope.",
    "",
    "Grant limits:",
    "max uses: " + settings.maxUses,
    "expires in seconds: " + Math.floor(settings.ttlMs / 1000),
    "",
    "Command examples:",
    examples.length ? examples.map((example) => "- " + example).join("\n") : "- Commands may vary, but must stay within the purpose and scope above.",
    "",
    "Raw command text is intentionally hidden from this approval prompt.",
  ].join("\n");
}

async function confirmExternalBashGrant(ctx, params, signal) {
  if (signal?.aborted) return false;
  let abortHandler;
  const abortPromise = signal ? new Promise((resolve) => {
    abortHandler = () => resolve(false);
    signal.addEventListener("abort", abortHandler, { once: true });
  }) : undefined;
  try {
    const confirmPromise = ctx.ui.confirm(
      "Approve external sandbox grant",
      externalBashApprovalMessage(params)
    );
    return abortPromise
      ? await Promise.race([confirmPromise, abortPromise])
      : await confirmPromise;
  } finally {
    if (signal && abortHandler) {
      signal.removeEventListener("abort", abortHandler);
    }
  }
}

async function ensureExternalBashGrant(ctx, params, signal) {
  const now = Date.now();
  pruneExternalBashGrants(now);
  const key = externalGrantKey(params);
  const existing = externalBashGrants.get(key);
  if (existing && existing.expiresAt > now && existing.uses < existing.maxUses) {
    existing.uses += 1;
    return true;
  }
  const approved = await confirmExternalBashGrant(ctx, params, signal);
  if (!approved) return false;
  recordExternalBashGrant(params, Date.now());
  return true;
}

function quoteStatefulCommandWord(word) {
  const value = String(word || "");
  if (/^[A-Za-z0-9_@%+=:,./-]+$/.test(value)) return value;
  return "'" + value.replace(/'/g, "'\\''") + "'";
}

function commandWordsToShell(words) {
  return words.map(quoteStatefulCommandWord).join(" ");
}

function flagValue(words, flag) {
  const index = words.indexOf(flag);
  if (index < 0) return undefined;
  return words[index + 1];
}

function insertSandboxIdentityFlag(words, flag, value) {
  if (flagValue(words, flag) !== undefined) return words;
  words.splice(3, 0, flag, value);
  return words;
}

function commandWithActiveSandboxIdentity(words, event, ctx) {
  if (!Array.isArray(words) || words[1] !== "sandbox" || words[2] !== "run") {
    return { words, command: undefined };
  }
  const activeAgentId = agentId(event, ctx);
  const suppliedAgentId = flagValue(words, "--agent-id");
  if (suppliedAgentId !== undefined && suppliedAgentId !== activeAgentId) {
    throw new Error("stateful sandbox run --agent-id must match the active agent_id");
  }
  const rewritten = [...words];
  insertSandboxIdentityFlag(rewritten, "--agent-id", activeAgentId);

  const activeWorkspaceId = detectWorkspaceId(event, ctx);
  const suppliedWorkspaceId = flagValue(words, "--workspace-id");
  if (activeWorkspaceId) {
    if (suppliedWorkspaceId !== undefined && suppliedWorkspaceId !== activeWorkspaceId) {
      throw new Error("stateful sandbox run --workspace-id must match the active workspace_id");
    }
    insertSandboxIdentityFlag(rewritten, "--workspace-id", activeWorkspaceId);
  }
  return { words: rewritten, command: commandWordsToShell(rewritten) };
}


function splitStatefulCommandWords(command) {
  const words = [];
  let current = "";
  let quote = null;
  for (let index = 0; index < command.length; index += 1) {
    const ch = command[index];
    if (quote) {
      if (ch === quote) {
        quote = null;
      } else {
        current += ch;
      }
      continue;
    }
    if (ch === "'" || ch === "\"") {
      quote = ch;
      continue;
    }
    if (ch === "\\" || ch === "`" || ch === "\n" || ch === "\r") {
      throw new Error("Bash wrapper must be a single stateful sandbox command");
    }
    if (ch === "$" && command[index + 1] === "(") {
      throw new Error("Bash wrapper must not use command substitution");
    }
    if (";|&<>".includes(ch)) {
      throw new Error("Bash wrapper must be a single stateful sandbox command");
    }
    if (/\s/.test(ch)) {
      if (current) {
        words.push(current);
        current = "";
      }
      continue;
    }
    current += ch;
  }
  if (quote) throw new Error("Bash wrapper command has unterminated quotes");
  if (current) words.push(current);
  return words;
}

function parseStatefulSandboxRunWords(words) {
  if (words.length < 4 || words[1] !== "sandbox" || words[2] !== "run") {
    return { allow: false, reason: "Bash commands must use stateful sandbox run" };
  }
  const params = {
    fs: "read-only",
    purpose: "",
    write_targets: [],
    create_targets: [],
    write_dirs: [],
    connect_sockets: [],
    allow_signal: false,
    network: undefined,
    agent_id: undefined,
    workspace_id: undefined,
    command: "",
    sequences: [],
    sequence_shell: undefined,
  };
  for (let index = 3; index < words.length; index += 1) {
    const arg = words[index];
    const nextValue = (name) => {
      index += 1;
      if (index >= words.length || !words[index]) throw new Error("stateful sandbox run " + name + " requires a value");
      return words[index];
    };
    if (arg === "--fs") params.fs = nextValue("--fs");
    else if (arg === "--purpose") params.purpose = nextValue("--purpose");
    else if (arg === "--write-target") params.write_targets.push(nextValue("--write-target"));
    else if (arg === "--create-target") params.create_targets.push(nextValue("--create-target"));
    else if (arg === "--write-dir") params.write_dirs.push(nextValue("--write-dir"));
    else if (arg === "--connect-socket") params.connect_sockets.push(nextValue("--connect-socket"));
    else if (arg === "--network") params.network = nextValue("--network");
    else if (arg === "--timeout-seconds") nextValue("--timeout-seconds");
    else if (arg === "--stream-events") continue;
    else if (arg === "--allow-signal") params.allow_signal = true;
    else if (arg === "--agent-id") params.agent_id = nextValue("--agent-id");
    else if (arg === "--workspace-id") params.workspace_id = nextValue("--workspace-id");
    else if (arg === "--command") params.command = nextValue("--command");
    else if (arg === "--sequence") params.sequences.push(nextValue("--sequence"));
    else if (arg === "--sequence-shell") {
      if (params.sequence_shell !== undefined) throw new Error("stateful sandbox run accepts at most one --sequence-shell");
      params.sequence_shell = nextValue("--sequence-shell");
    }
    else throw new Error("unsupported stateful sandbox run argument `" + arg + "`");
  }
  const hasCommand = Boolean(params.command);
  const hasSequence = params.sequences.length > 0;
  if (hasCommand && hasSequence) {
    return { allow: false, reason: "stateful sandbox run accepts either --command or --sequence, not both" };
  }
  if (!hasCommand && !hasSequence) {
    return { allow: false, reason: "stateful sandbox run requires exactly one --command or at least one --sequence" };
  }
  if (params.sequence_shell !== undefined && !hasSequence) {
    return { allow: false, reason: "stateful sandbox run --sequence-shell requires --sequence" };
  }
  if (params.sequence_shell !== undefined && !/^\//.test(params.sequence_shell)) {
    return { allow: false, reason: "stateful sandbox run --sequence-shell requires an absolute shell path" };
  }
  if (hasSequence && params.fs === "git") {
    return { allow: false, reason: "git profile requires a single git command" };
  }
  if (hasSequence && params.fs === "github-pr") {
    return { allow: false, reason: "github-pr profile requires a single gh pr command" };
  }
  if (params.fs === "external" && !params.purpose.trim()) {
    return { allow: false, reason: "stateful sandbox run --fs external requires --purpose" };
  }
  if (params.fs === "external" && (hasExternalWriteScope(params) || stringList(params.connect_sockets).length > 0 || params.allow_signal === true)) {
    return { allow: true, externalGrantParams: params };
  }
  return { allow: true };
}

function parseStatefulProcessFindWords(words) {
  if (words.length < 5 || words[1] !== "sandbox" || words[2] !== "process" || words[3] !== "find") {
    return { allow: false, reason: "Bash commands must use stateful sandbox process find" };
  }
  return { allow: true };
}
function statefulBashPassthroughDecision(command, cwd) {
  try {
    const words = splitStatefulCommandWords(String(command || "").trim());
    if (words.length === 0) return { allow: false, reason: "Bash command is empty" };
    if (!isTrustedStatefulCommand(words[0], cwd)) {
      return { allow: false, reason: "OMP raw Bash is denied; use the trusted stateful sandbox command" };
    }
    let decision;
    if (words[1] === "sandbox" && words[2] === "run") decision = parseStatefulSandboxRunWords(words);
    else if (words[1] === "sandbox" && words[2] === "process" && words[3] === "find") decision = parseStatefulProcessFindWords(words);
    else decision = { allow: false, reason: "Bash commands must use stateful sandbox run or stateful sandbox process find" };
    if (decision.allow) decision.words = words;
    return decision;
  } catch (error) {
    return { allow: false, reason: error instanceof Error ? error.message : String(error) };
  }
}


export default function statefulOmpExtension(pi) {
  pi.setLabel("Stateful");
  pi.on("tool_call", async (event, ctx) => {
    if (event?.toolName !== "bash" && event?.toolName !== "functions.bash") return;
    const decision = statefulBashPassthroughDecision(event?.input?.command, ctx?.cwd);
    if (!decision.allow) return { block: true, reason: decision.reason };
    try {
      const rewritten = commandWithActiveSandboxIdentity(decision.words, event, ctx);
      decision.words = rewritten.words;
      if (rewritten.command && event?.input) event.input.command = rewritten.command;
    } catch (error) {
      return { block: true, reason: error instanceof Error ? error.message : String(error) };
    }
    if (decision.externalGrantParams) {
      const params = decision.externalGrantParams;
      if (shouldAutoApproveStatefulPrompt(ctx, params)) {
        approveExternalBashGrantWithoutPrompt(params);
        return;
      }
      if (typeof ctx?.ui?.confirm !== "function") {
        const operationId = rememberLazyBashOperation(event, ctx, decision);
        const suffix = operationId
          ? "\n\nQueued lazy bash operation_id: " + operationId + "\nNext: approve the external sandbox grant, then call lazy_bash_resume with this operation_id."
          : "";
        return { block: true, reason: "Built-in Bash external sandbox command requires OMP UI confirmation; use stateful.autoApprove to skip this prompt." + suffix };
      }
      const signal = undefined;
      const approved = await ensureExternalBashGrant(ctx, params, signal);
      if (!approved) return { block: true, reason: "user denied stateful external sandbox grant" };
    }
  });
function state_reservation_claim(operation, _ctx) {
  const waitId = String(operation?.wait_id || "").trim();
  if (!waitId) return { ok: true };
  if (operation?.claimable !== true || !String(operation?.reservation_id || "").trim()) {
    return { ok: false, message: "state_reservation_claim wait is not claimable yet" };
  }
  return { ok: true };
}

  pi.registerTool({
    name: "lazy_edit_resume",
    label: "Lazy Edit Resume",
    description: "Resume a blocked OMP edit operation after the needed reservation or claim is ready. Applies only strict line-based OMP edit patches captured in this live extension session.",
    parameters: {
      type: "object",
      properties: {
        operation_id: { type: "string", description: "Queued lazy edit operation id; either a Stateful wait_id or a generated live-session id printed in the block message." },
      },
      required: ["operation_id"],
    },
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      const operationId = String(params?.operation_id || "").trim();
      const operation = lazyEditOperations.get(operationId);
      if (!operation) {
        return lazyToolResult("failed", "lazy edit operation not found in this live OMP extension session", { operation_id: operationId });
      }
      const claim = state_reservation_claim(operation, ctx);
      if (!claim.ok) return lazyToolResult("failed", claim.message, { operation_id: operationId, targets: operation.targets });
      const authorization = runStatefulHook("pre-tool-use", {
        tool_call_id: operation.tool_call_id,
        wait_id: operation.wait_id || undefined,
        agent_id: operation.agent_id,
        reservation_id: operation.reservation_id || undefined,
        cwd: operation.cwd || ctx.cwd,
        yolo: true,
        tool_name: operation.tool_name,
        tool_input: operation.tool_input,
      });
      if (authorization.decision !== "allow") {
        return lazyToolResult("failed", authorization.reason || "stateful authorization denied lazy edit resume", { operation_id: operationId, authorization });
      }
      let result;
      try {
        result = applyOmpLinePatch(operation.cwd || ctx.cwd, operation.tool_input?.input || "", operation.bases);
      } catch (error) {
        result = { status: "failed", message: error instanceof Error ? error.message : String(error) };
      }
      if (result.status === "applied") lazyEditOperations.delete(operationId);
      runStatefulHook("post-tool-use", {
        agent_id: operation.agent_id,
        tool_call_id: operation.tool_call_id,
        reservation_id: operation.reservation_id || undefined,
        wait_id: operation.wait_id || undefined,
        cwd: operation.cwd || ctx.cwd,
        tool_name: operation.tool_name,
        tool_input: operation.tool_input,
        is_error: result.status !== "applied",
        is_complete: true,
        exact_read_candidate: false,
        result_metadata: {
          status: result.status,
          message: result.message,
          targets: operation.targets,
          wait_id: operation.wait_id || undefined,
          reservation_id: operation.reservation_id || undefined,
        },
      });
      return lazyToolResult(result.status, result.message, { operation_id: operationId, targets: operation.targets });
    },
  });
  pi.registerTool({
    name: "lazy_write_resume",
    label: "Lazy Write Resume",
    description: "Resume a blocked OMP write operation after the needed reservation or claim is ready. Replays only write operations captured in this live extension session and fails if the target changed while queued.",
    parameters: {
      type: "object",
      properties: {
        operation_id: { type: "string", description: "Queued lazy write operation id; either a Stateful wait_id or a generated live-session id printed in the block message." },
      },
      required: ["operation_id"],
    },
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      const operationId = String(params?.operation_id || "").trim();
      const operation = lazyWriteOperations.get(operationId);
      if (!operation) {
        return lazyToolResult("failed", "lazy write operation not found in this live OMP extension session", { operation_id: operationId });
      }
      const claim = state_reservation_claim(operation, ctx);
      if (!claim.ok) return lazyToolResult("failed", claim.message, { operation_id: operationId, targets: operation.targets });
      const authorization = runStatefulHook("pre-tool-use", {
        agent_id: operation.agent_id,
        reservation_id: operation.reservation_id || undefined,
        cwd: operation.cwd || ctx.cwd,
        yolo: true,
        tool_call_id: operation.tool_call_id,
        wait_id: operation.wait_id || undefined,
        tool_name: operation.tool_name,
        tool_input: operation.tool_input,
      });
      if (authorization.decision !== "allow") {
        return lazyToolResult("failed", authorization.reason || "stateful authorization denied lazy write resume", { operation_id: operationId, authorization });
      }
      let result;
      try {
        result = applyOmpWrite(operation.cwd || ctx.cwd, operation);
      } catch (error) {
        result = { status: "failed", message: error instanceof Error ? error.message : String(error) };
      }
      if (result.status === "applied") lazyWriteOperations.delete(operationId);
      runStatefulHook("post-tool-use", {
        agent_id: operation.agent_id,
        tool_call_id: operation.tool_call_id,
        reservation_id: operation.reservation_id || undefined,
        wait_id: operation.wait_id || undefined,
        cwd: operation.cwd || ctx.cwd,
        tool_name: operation.tool_name,
        tool_input: operation.tool_input,
        is_error: result.status !== "applied",
        is_complete: true,
        exact_read_candidate: false,
        result_metadata: {
          status: result.status,
          message: result.message,
          wait_id: operation.wait_id || undefined,
          reservation_id: operation.reservation_id || undefined,
          targets: operation.targets,
        },
      });
      return lazyToolResult(result.status, result.message, { operation_id: operationId, targets: operation.targets });
    },
  });
  pi.registerTool({
    name: "lazy_bash_resume",
    label: "Lazy Bash Resume",
    description: "Resume a blocked OMP Bash command after approving an external sandbox grant.",
    parameters: {
      type: "object",
      properties: {
        operation_id: { type: "string", description: "Queued lazy bash operation id printed in the block message." },
      },
      required: ["operation_id"],
    },
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {
      const operationId = String(params?.operation_id || "").trim();
      const operation = lazyBashOperations.get(operationId);
      if (!operation) {
        return lazyToolResult("failed", "lazy bash operation not found in this live OMP extension session", { operation_id: operationId });
      }
      if (typeof ctx?.ui?.confirm !== "function" && !shouldAutoApproveStatefulPrompt(ctx, operation.grant_params)) {
        return lazyToolResult("failed", "lazy bash resume requires OMP UI confirmation or stateful.autoApprove", { operation_id: operationId });
      }
      const approved = shouldAutoApproveStatefulPrompt(ctx, operation.grant_params)
        ? approveExternalBashGrantWithoutPrompt(operation.grant_params)
        : await ensureExternalBashGrant(ctx, operation.grant_params, signal);
      if (!approved) {
        return lazyToolResult("failed", "user denied stateful external sandbox grant", { operation_id: operationId });
      }
      const authorization = runStatefulHook("pre-tool-use", {
        agent_id: operation.agent_id,
        cwd: operation.cwd || ctx.cwd,
        yolo: true,
        tool_name: operation.tool_name,
        tool_input: operation.tool_input,
      });
      if (authorization.decision !== "allow") {
        return lazyToolResult("failed", authorization.reason || "stateful authorization denied lazy bash resume", { operation_id: operationId, authorization });
      }
      const words = operation.command_words || [];
      const result = spawnSync(words[0], words.slice(1), {
        cwd: operation.cwd || ctx.cwd,
        encoding: "utf8",
      });
      if (result.status === 0) {
        lazyBashOperations.delete(operationId);
        runStatefulHook("post-tool-use", {
          agent_id: operation.agent_id,
          cwd: operation.cwd || ctx.cwd,
          tool_name: operation.tool_name,
          tool_input: operation.tool_input,
        });
      }
      return lazyToolResult(result.status === 0 ? "applied" : "failed", bashResumeText(result), { operation_id: operationId, exit_code: result.status });
    },
  });
  pi.on("session_start", async (event, ctx) => {
    verifyBareStateful(ctx.cwd);
    const activeAgentId = detectAgentId(event, ctx);
    if (!activeAgentId) {
      stopContextStream();
      return;
    }
    const result = runStatefulHook("session-start", {
      agent_id: activeAgentId,
      cwd: ctx.cwd,
    });
    const stream = {
      ...result?.notifications_stream,
      agent_id: firstString(result?.notifications_stream?.agent_id, result?.agent_id, activeAgentId),
      workspace_id: firstString(result?.notifications_stream?.workspace_id, result?.workspace_id, detectWorkspaceId(event, ctx)),
      cwd: ctx.cwd,
    };
    if (!stream.base_url || !stream.authorization || !stream.agent_id || !stream.workspace_id) {
      stopContextStream();
      return;
    }
    activateContextStream(stream, true);
    if (!await deliverContext(pi, stream)) contextState.initialPending = true;
    startContextStream(pi, stream);
  });
  pi.on("tool_call", async (event, ctx) => {
    const benchmarkBlockReason = benchmarkSourceBlockReason(event);
    if (benchmarkBlockReason) return { block: true, reason: benchmarkBlockReason };
    const activeAgentId = detectAgentId(event, ctx);
    if (!activeAgentId) return { block: true, reason: missingAgentIdReason() };
    const decision = runStatefulHook("pre-tool-use", {
      agent_id: activeAgentId,
      tool_call_id: event?.toolCallId,
      reservation_id: reservationId(event),
      cwd: ctx.cwd,
      yolo: isYolo(event, ctx),
      tool_name: event.toolName,
      tool_input: event.input || {},
    });
    if (decision.decision === "prompt" && !shouldAutoApproveStatefulPrompt(ctx, event.input || {})) {
      if (typeof ctx?.ui?.confirm !== "function") {
        return {
          block: true,
          reason: "Stateful requested approval, but OMP UI confirmation is unavailable.",
        };
      }
      const approved = await ctx.ui.confirm(
        decision.title || "Approve stateful action",
        decision.message || decision.reason || "Approve this stateful action?"
      );
      if (!approved) {
        return { block: true, reason: decision.reason || "Blocked by user" };
      }
    }
    if (decision.decision === "warn") {
      if (typeof pi?.sendMessage === "function") {
        pi.sendMessage(
          {
            customType: "stateful_coordination_warning",
            content: decision.message,
            display: true,
          },
          { deliverAs: "nextTurn" }
        );
      }
      return;
    }
    if (decision.decision === "block") {
      const editOperationId = rememberLazyEditOperation(event, ctx, decision);
      const writeOperationId = rememberLazyWriteOperation(event, ctx, decision);
      const suffix = editOperationId
        ? "\n\nQueued lazy edit operation_id: " + editOperationId + "\nNext: when reservation or claim is ready, call lazy_edit_resume with this operation_id."
        : writeOperationId
          ? "\n\nQueued lazy write operation_id: " + writeOperationId + "\nNext: when reservation or claim is ready, call lazy_write_resume with this operation_id."
          : "";
      return { block: true, reason: decision.reason + suffix };
    }
  });
  pi.on("tool_result", async (event, ctx) => {
    const activeAgentId = detectAgentId(event, ctx);
    if (!activeAgentId) return;
    const resultMetadata = event?.resultMetadata
      ?? event?.result_metadata
      ?? event?.metadata
      ?? event?.details
      ?? event?.result
      ?? {};
    runStatefulHook("post-tool-use", {
      agent_id: activeAgentId,
      tool_call_id: event?.toolCallId,
      cwd: ctx.cwd,
      tool_name: event.toolName,
      tool_input: event.input || {},
      is_error: event?.isError === true,
      is_complete: event?.isComplete !== false && event?.complete !== false,
      exact_read_candidate: exactReadCandidate(event),
      result_metadata: resultMetadata,
    });
    if (activeContextStream) {
      await recoverContextNotifications(pi, activeContextStream);
    }
  });
  pi.on("session_shutdown", async (event, ctx) => {
    stopContextStream();
    const activeAgentId = detectAgentId(event, ctx);
    if (!activeAgentId) return;
    runStatefulHook("stop", {
      agent_id: activeAgentId,
      cwd: ctx.cwd,
    });
  });
};
