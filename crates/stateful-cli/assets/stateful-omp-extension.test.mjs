import assert from "node:assert/strict";
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const assetPath = fileURLToPath(new URL("./stateful-omp-extension.js", import.meta.url));
const assetSource = await readFile(assetPath, "utf8");

function jsonResponse(body, ok = true) {
  return { ok, json: async () => body };
}
function openStream() {
  return { ok: true, body: { getReader: () => ({ read: () => new Promise(() => {}) }) } };
}


function fakePi() {
  const handlers = new Map();
  const tools = new Map();
  const messages = [];
  return {
    handlers,
    messages,
    tools,
    on(name, handler) { handlers.set(name, handler); },
    registerTool(tool) { tools.set(tool.name, tool); },
    sendMessage(message, options) { messages.push({ message, options }); },
    setLabel() {},
  };
}

async function loadExtension(t, { decision = { decision: "allow" }, yoloDecision = { decision: "allow" }, fetchImpl, claimRequiresPath = false, claimAlwaysFails = false } = {}) {
  const directory = await mkdtemp(join(tmpdir(), "stateful-omp-extension-"));
  await mkdir(join(directory, ".git"));
  const extensionContext = context(directory);
  const hookLog = join(directory, "hooks.jsonl");
  const statefulPath = join(directory, "stateful");
  const modulePath = join(directory, "stateful-omp-extension.mjs");
  const runner = `#!/usr/bin/env node
const { appendFileSync } = require("node:fs");
let input = "";
process.stdin.on("data", (chunk) => { input += chunk; });
process.stdin.on("end", () => {
  const event = process.argv[4];
  const payload = JSON.parse(input || "{}");
  appendFileSync(${JSON.stringify(hookLog)}, JSON.stringify({ event, args: process.argv.slice(2), payload }) + "\\n");
  if (${JSON.stringify(claimAlwaysFails)} && process.argv[2] === "reservation" && process.argv[3] === "claim") {
    process.stderr.write("claim must not be repeated after a grant");
    process.exitCode = 1;
    return;
  }
  const decision = ${JSON.stringify(decision)};
  if (${JSON.stringify(claimRequiresPath)} && process.argv[2] === "reservation" && process.argv[3] === "claim" && !process.argv.includes("--path")) {
    process.stderr.write("reservation claim requires --path");
    process.exitCode = 1;
    return;
  }
  if (event === "session-start") {
    process.stdout.write(JSON.stringify({
      decision: "allow",
      notifications_stream: {
        base_url: "http://stateful.test",
        authorization: "Bearer test-token",
        agent_id: payload.agent_id,
        workspace_id: "workspace-1",
        root: "/workspace",
        repo_id: "repo-1",
        worktree_id: "worktree-1",
        branch: "main",
      },
    }));
    return;
  }
  process.stdout.write(JSON.stringify(event === "pre-tool-use" && payload.yolo ? ${JSON.stringify(yoloDecision)} : decision));
});
`;
  await writeFile(statefulPath, runner, { mode: 0o755 });
  await chmod(statefulPath, 0o755);
  await writeFile(modulePath, assetSource.replace("__STATEFUL_BINARY_JSON__", JSON.stringify(statefulPath)));
  const previousFetch = globalThis.fetch;
  globalThis.fetch = fetchImpl || (async () => { throw new Error("unexpected fetch"); });
  const extension = await import(`${pathToFileURL(modulePath).href}?${Date.now()}-${Math.random()}`);
  const pi = fakePi();
  extension.default(pi);
  t.after(async () => {
    const shutdown = pi.handlers.get("session_shutdown");
    if (shutdown) {
      await shutdown({}, extensionContext);
    }
    globalThis.fetch = previousFetch;
    await rm(directory, { recursive: true, force: true });
  });
  return { extension, pi, hookLog, context: extensionContext, directory };
}

function context(cwd = "/workspace") {
  return {
    cwd,
    workspaceId: "workspace-1",
    sessionManager: {
      getSessionId: () => "00000000-0000-4000-8000-000000000011",
    },
  };
}

async function hooks(hookLog) {
  try {
    return (await readFile(hookLog, "utf8"))
      .trim()
      .split("\n")
      .filter(Boolean)
      .map(JSON.parse);
  } catch {
    return [];
  }
}

test("exactReadCandidate accepts only full raw, no-range, non-truncated reads", async (t) => {
  const { extension } = await loadExtension(t);
  const candidate = {
    toolName: "functions.read",
    input: { path: "src/lib.rs:raw" },
    isError: false,
    isComplete: true,
    result: { truncated: false },
  };
  assert.equal(extension.exactReadCandidate(candidate), true);
  assert.equal(extension.exactReadCandidate({ ...candidate, input: { path: "src/lib.rs:10-20:raw" } }), false);
  assert.equal(extension.exactReadCandidate({ ...candidate, input: { path: "src/lib.rs:50-:raw" } }), false);
  assert.equal(extension.exactReadCandidate({ ...candidate, input: { path: "src/lib.rs" } }), false);
  assert.equal(extension.exactReadCandidate({ ...candidate, result: { truncated: true } }), false);
});

test("coalesceContextInvalidation retains the latest valid target version", async (t) => {
  const { extension } = await loadExtension(t);
  assert.equal(extension.coalesceContextInvalidation(undefined, 4), 4);
  assert.equal(extension.coalesceContextInvalidation(4, 9), 9);
  assert.equal(extension.coalesceContextInvalidation(9, 4), 9);
  assert.equal(extension.coalesceContextInvalidation(9, "not-a-version"), 9);
});

test("shouldDeliverContextVersion suppresses duplicate and out-of-order versions", async (t) => {
  const { extension } = await loadExtension(t);
  assert.equal(extension.shouldDeliverContextVersion(undefined, 1), true);
  assert.equal(extension.shouldDeliverContextVersion(3, 4), true);
  assert.equal(extension.shouldDeliverContextVersion(4, 4), false);
  assert.equal(extension.shouldDeliverContextVersion(4, 3), false);
});

test("session start queues initial context for the next turn then acknowledges it", async (t) => {
  const calls = [];
  const { pi } = await loadExtension(t, {
    fetchImpl: async (url, options = {}) => {
      calls.push({ url: String(url), options });
      if (String(url).endsWith("/v2/context/render")) {
        return jsonResponse({ changed: true, delivery_id: "delivery-1", sequence: 1, workspace_version: 1, prompt_text: "Initial context" });
      }
      if (String(url).endsWith("/v2/context/ack")) return jsonResponse({ acknowledged_version: 1, cursor: 1 });
      if (String(url).endsWith("/v2/notifications/stream")) return openStream();
      throw new Error(`unexpected URL ${url}`);
    },
  });
  await pi.handlers.get("session_start")({ workspaceId: "workspace-1" }, context());
  assert.deepEqual(pi.messages, [{
    message: { customType: "stateful_context", content: "Initial context", display: true },
    options: { triggerTurn: false, deliverAs: "nextTurn" },
  }]);
  assert.deepEqual(calls.map(({ url }) => new URL(url).pathname).slice(0, 2), ["/v2/context/render", "/v2/context/ack"]);
});

test("a new session does not suppress an unacknowledged equal-version context", async (t) => {
  let acknowledgements = 0;
  const { pi } = await loadExtension(t, {
    fetchImpl: async (url) => {
      const pathname = new URL(url).pathname;
      if (pathname === "/v2/context/render") {
        return jsonResponse({ changed: true, delivery_id: "delivery-session", sequence: 1, workspace_version: 1, prompt_text: "Session context" });
      }
      if (pathname === "/v2/context/ack") {
        acknowledgements += 1;
        return jsonResponse({ acknowledged_version: 1, cursor: 1 });
      }
      if (pathname === "/v2/notifications/stream") return openStream();
      throw new Error(`unexpected URL ${url}`);
    },
  });
  await pi.handlers.get("session_start")({ workspaceId: "workspace-1" }, context());
  await pi.handlers.get("session_shutdown")({}, context());
  await pi.handlers.get("session_start")({ workspaceId: "workspace-1" }, context());
  assert.equal(pi.messages.filter(({ message }) => message.customType === "stateful_context").length, 2);
  assert.equal(acknowledgements, 2);
});

test("a failed context acknowledgement is redelivered on the next tool result", async (t) => {
  let acknowledgements = 0;
  const { pi } = await loadExtension(t, {
    fetchImpl: async (url) => {
      const pathname = new URL(url).pathname;
      if (pathname === "/v2/context/render") {
        return jsonResponse({ changed: true, delivery_id: "delivery-2", sequence: 2, workspace_version: 2, prompt_text: "Retry context" });
      }
      if (pathname === "/v2/context/ack") {
        acknowledgements += 1;
        return jsonResponse({}, acknowledgements > 1);
      }
      if (pathname === "/v2/notifications/poll") return jsonResponse([]);
      if (pathname === "/v2/notifications/stream") return openStream();
      throw new Error(`unexpected URL ${url}`);
    },
  });
  await pi.handlers.get("session_start")({ workspaceId: "workspace-1" }, context());
  await pi.handlers.get("tool_result")({ toolCallId: "call-2", toolName: "functions.read", input: { path: "src/lib.rs:raw" } }, context());
  assert.equal(acknowledgements, 2);
  assert.equal(pi.messages.filter(({ message }) => message.customType === "stateful_context").length, 2);
});

test("tool results recover a missed context invalidation through V2 notifications", async (t) => {
  let polled = false;
  const calls = [];
  const { pi } = await loadExtension(t, {
    fetchImpl: async (url) => {
      const pathname = new URL(url).pathname;
      calls.push(pathname);
      if (pathname === "/v2/context/render") {
        return jsonResponse(polled
          ? { changed: true, delivery_id: "delivery-4", sequence: 4, workspace_version: 4, prompt_text: "Recovered context" }
          : { changed: false, workspace_version: 1, prompt_text: "" });
      }
      if (pathname === "/v2/context/ack") return jsonResponse({ acknowledged_version: 4, cursor: 4 });
      if (pathname === "/v2/notifications/poll") {
        if (polled) return jsonResponse([]);
        polled = true;
        return jsonResponse([{
          notification_id: "notification-4",
          sequence: 4,
          kind: "context_invalidated",
          payload: { target_version: 4 },
        }]);
      }
      if (pathname === "/v2/notifications/stream") return openStream();
      throw new Error(`unexpected URL ${url}`);
    },
  });
  await pi.handlers.get("session_start")({ workspaceId: "workspace-1" }, context());
  calls.length = 0;
  await pi.handlers.get("tool_result")({ toolCallId: "call-4", toolName: "bash", input: { command: "true" } }, context());
  assert.ok(calls.includes("/v2/notifications/poll"));
  const recovered = pi.messages.at(-1);
  assert.equal(recovered.message.customType, "stateful_context");
  assert.equal(recovered.message.content, "Recovered context");
  assert.deepEqual(recovered.options, { triggerTurn: true, deliverAs: "nextTurn" });
});

test("SSE notification transport dispatches payload kinds before acknowledgement", async (t) => {
  let contextRenders = 0;
  let reads = 0;
  const acknowledged = [];
  let resolveAcknowledgements;
  const acknowledgements = new Promise((resolve) => { resolveAcknowledgements = resolve; });
  const frame = [
    "event: notification",
    "data: {\"notification_id\":\"notification-2\",\"sequence\":2,\"kind\":\"context_invalidated\",\"payload\":{\"target_version\":2}}",
    "",
    "event: notification",
    "data: {\"notification_id\":\"notification-3\",\"sequence\":3,\"kind\":\"reservation_granted\",\"payload\":{\"relative_path\":\"src/lib.rs\",\"wait_id\":\"wait-3\"}}",
    "",
    "",
  ].join("\n");
  const { pi } = await loadExtension(t, {
    fetchImpl: async (url, options = {}) => {
      const pathname = new URL(url).pathname;
      if (pathname === "/v2/context/render") {
        contextRenders += 1;
        return jsonResponse(contextRenders === 1
          ? { changed: false, workspace_version: 1, prompt_text: "" }
          : { changed: true, delivery_id: "delivery-2", sequence: 2, workspace_version: 2, prompt_text: "Stream context" });
      }
      if (pathname === "/v2/context/ack") return jsonResponse({ acknowledged_version: 2, cursor: 2 });
      if (pathname === "/v2/notifications/poll") {
        const sequence = JSON.parse(options.body).payload.sequence;
        acknowledged.push({
          sequence,
          contextDelivered: pi.messages.some(({ message }) => message.customType === "stateful_context"),
          reservationDelivered: pi.messages.some(({ message }) => message.customType === "stateful_reservation_ready"),
        });
        if (acknowledged.length === 2) resolveAcknowledgements();
        return jsonResponse({});
      }
      if (pathname === "/v2/notifications/stream") {
        return {
          ok: true,
          body: {
            getReader: () => ({
              read: async () => {
                if (reads++ === 0) return { done: false, value: new TextEncoder().encode(frame) };
                return new Promise(() => {});
              },
            }),
          },
        };
      }
      throw new Error(`unexpected URL ${url}`);
    },
  });
  await pi.handlers.get("session_start")({ workspaceId: "workspace-1" }, context());
  await acknowledgements;
  assert.equal(contextRenders, 2);
  assert.deepEqual(acknowledged, [
    { sequence: 2, contextDelivered: true, reservationDelivered: false },
    { sequence: 3, contextDelivered: true, reservationDelivered: true },
  ]);
  const streamedContext = pi.messages.find(({ message }) => message.customType === "stateful_context");
  const streamedReservation = pi.messages.find(({ message }) => message.customType === "stateful_reservation_ready");
  assert.deepEqual(streamedContext.options, { triggerTurn: true, deliverAs: "nextTurn" });
  assert.deepEqual(streamedReservation.options, { triggerTurn: true, deliverAs: "nextTurn" });
});

test("awareness warnings are queued for the next turn without a lazy write", async (t) => {
  const { pi } = await loadExtension(t, { decision: { decision: "warn", message: "overlapping work" } });
  const result = await pi.handlers.get("tool_call")({ toolCallId: "call-warn", toolName: "functions.write", input: { path: "src/lib.rs", content: "x" } }, context());
  assert.equal(result, undefined);
  assert.deepEqual(pi.messages, [{
    message: { customType: "stateful_coordination_warning", content: "overlapping work", display: true },
    options: { deliverAs: "nextTurn" },
  }]);
  const lazy = await pi.tools.get("lazy_write_resume").execute("call-warn", { operation_id: "call-warn" }, undefined, undefined, context());
  assert.equal(lazy.details.operation_id, "call-warn");
  assert.match(lazy.content[0].text, /not found/);
});

test("enforcement denials retain lazy write replay", async (t) => {
  const { pi, context: extensionContext } = await loadExtension(t, { decision: { decision: "block", reason: "reservation pending", wait: { wait_id: "wait-8" } } });
  const result = await pi.handlers.get("tool_call")({ toolCallId: "call-block", toolName: "write", input: { path: "src/lib.rs", content: "x" } }, extensionContext);
  assert.equal(result.block, true);
  assert.match(result.reason, /Queued lazy write operation_id: wait-8/);
});

test("pre and post hooks retain the tool call ID and result metadata", async (t) => {
  const { pi, hookLog } = await loadExtension(t);
  const event = {
    toolCallId: "call-correlation",
    toolName: "functions.read",
    input: { path: "src/lib.rs:raw" },
    isError: false,
    isComplete: true,
    resultMetadata: { truncated: false, bytes: 12 },
  };
  await pi.handlers.get("tool_call")(event, context());
  await pi.handlers.get("tool_result")(event, context());
  const recorded = await hooks(hookLog);
  const pre = recorded.find(({ event: name }) => name === "pre-tool-use").payload;
  const post = recorded.find(({ event: name }) => name === "post-tool-use").payload;
  assert.equal(pre.tool_call_id, "call-correlation");
  assert.equal(post.tool_call_id, "call-correlation");
  assert.equal(post.is_error, false);
  assert.equal(post.is_complete, true);
  assert.equal(post.exact_read_candidate, true);
  assert.deepEqual(post.result_metadata, { truncated: false, bytes: 12 });
});

test("truncated raw OMP reads report top-level truncation", async (t) => {
  const { pi, hookLog } = await loadExtension(t);
  const event = {
    toolCallId: "call-truncated",
    toolName: "functions.read",
    input: { path: "src/lib.rs:raw" },
    isError: false,
    isComplete: true,
    resultMetadata: { truncated: true },
  };
  await pi.handlers.get("tool_call")(event, context());
  await pi.handlers.get("tool_result")(event, context());
  const recorded = await hooks(hookLog);
  const post = recorded.find(({ event: name }) => name === "post-tool-use").payload;
  assert.equal(post.exact_read_candidate, false);
  assert.equal(post.is_truncated, true);
});

test("lazy edit resume preserves the original tool-call identity through both hooks", async (t) => {
  const { extension, pi, hookLog, context: extensionContext, directory } = await loadExtension(t, {
    decision: {
      decision: "block",
      reason: "reservation pending",
      wait: { wait_id: "wait-edit", reservation_id: "reservation-edit" },
    },
    claimRequiresPath: true,
  });
  await writeFile(join(directory, "resume-edit.js"), "before\n");
  const input = { input: "[resume-edit.js#0000]\nSWAP 1.=1:\n+after" };

  const blocked = await pi.handlers.get("tool_call")({
    toolCallId: "original-edit-call",
    toolName: "edit",
    reservationId: "reservation-edit",
    input,
  }, extensionContext);
  assert.equal(blocked.block, true);
  assert.equal(extension.bindGrantedLazyReservation({
    kind: "reservation_granted",
    payload: { wait_id: "wait-edit", reservation_id: "reservation-edit" },
  }), true);

  const resumed = await pi.tools.get("lazy_edit_resume").execute(
    "resume-edit-tool-call",
    { operation_id: "wait-edit" },
    undefined,
    undefined,
    extensionContext,
  );
  assert.equal(resumed.isError, false);
  assert.equal(await readFile(join(directory, "resume-edit.js"), "utf8"), "after\n");

  const recorded = await hooks(hookLog);
  const resumedHooks = recorded.filter(({ event, payload }) => event === "pre-tool-use" && payload.yolo)
    .concat(recorded.filter(({ event, payload }) => event === "post-tool-use" && payload.tool_call_id === "original-edit-call"));
  assert.equal(resumedHooks.length, 2);
  for (const { event, payload } of resumedHooks) {
    assert.equal(payload.tool_call_id, "original-edit-call");
    assert.equal(payload.wait_id, undefined);
    assert.equal(payload.reservation_id, event === "pre-tool-use" ? "reservation-edit" : undefined);
    assert.deepEqual(payload.tool_input, input);
  }
  assert.equal(resumedHooks[1].payload.is_error, false);
  assert.equal(resumedHooks[1].payload.is_complete, true);
  assert.deepEqual(resumedHooks[1].payload.result_metadata, {
    status: "applied",
    message: "lazy edit applied",
    targets: ["resume-edit.js"],
    wait_id: "wait-edit",
    reservation_id: "reservation-edit",
  });
});

test("lazy write resume preserves the original tool-call identity through both hooks", async (t) => {
  const { extension, pi, hookLog, context: extensionContext, directory } = await loadExtension(t, {
    decision: {
      decision: "block",
      reason: "reservation pending",
      wait: { wait_id: "wait-write", reservation_id: "reservation-write" },
    },
    claimRequiresPath: true,
  });
  await writeFile(join(directory, "resume-write.js"), "before\n");
  const input = { path: "resume-write.js", content: "after\n" };

  const blocked = await pi.handlers.get("tool_call")({
    toolCallId: "original-write-call",
    toolName: "write",
    reservationId: "reservation-write",
    input,
  }, extensionContext);
  assert.equal(blocked.block, true);
  assert.equal(extension.bindGrantedLazyReservation({
    kind: "reservation_granted",
    payload: { wait_id: "wait-write", reservation_id: "reservation-write" },
  }), true);

  const resumed = await pi.tools.get("lazy_write_resume").execute(
    "resume-write-tool-call",
    { operation_id: "wait-write" },
    undefined,
    undefined,
    extensionContext,
  );
  assert.equal(resumed.isError, false);
  assert.equal(await readFile(join(directory, "resume-write.js"), "utf8"), "after\n");

  const recorded = await hooks(hookLog);
  const resumedHooks = recorded.filter(({ event, payload }) => event === "pre-tool-use" && payload.yolo)
    .concat(recorded.filter(({ event, payload }) => event === "post-tool-use" && payload.tool_call_id === "original-write-call"));
  assert.equal(resumedHooks.length, 2);
  for (const { event, payload } of resumedHooks) {
    assert.equal(payload.tool_call_id, "original-write-call");
    assert.equal(payload.wait_id, undefined);
    assert.equal(payload.reservation_id, event === "pre-tool-use" ? "reservation-write" : undefined);
    assert.deepEqual(payload.tool_input, input);
  }
  assert.equal(resumedHooks[1].payload.is_error, false);
  assert.equal(resumedHooks[1].payload.is_complete, true);
  assert.deepEqual(resumedHooks[1].payload.result_metadata, {
    status: "applied",
    message: "lazy write applied",
    targets: ["resume-write.js"],
    wait_id: "wait-write",
    reservation_id: "reservation-write",
  });
});

test("lazy write resume executes through a yolo warning and completes the original call", async (t) => {
  const { extension, pi, hookLog, context: extensionContext, directory } = await loadExtension(t, {
    decision: { decision: "block", reason: "reservation pending", wait: { wait_id: "wait-warn", reservation_id: "reservation-warn" } },
    yoloDecision: { decision: "warn", message: "overlapping work" },
    claimRequiresPath: true,
  });
  const input = { path: "resume-warn.js", content: "after\n" };
  const blocked = await pi.handlers.get("tool_call")({
    toolCallId: "original-warn-call",
    toolName: "write",
    reservationId: "reservation-warn",
    input,
  }, extensionContext);
  assert.equal(blocked.block, true);
  assert.equal(extension.bindGrantedLazyReservation({
    kind: "reservation_granted",
    payload: { wait_id: "wait-warn", reservation_id: "reservation-warn" },
  }), true);

  const resumed = await pi.tools.get("lazy_write_resume").execute(
    "resume-warn-tool-call",
    { operation_id: "wait-warn" },
    undefined,
    undefined,
    extensionContext,
  );
  assert.equal(resumed.isError, false);
  assert.equal(await readFile(join(directory, "resume-warn.js"), "utf8"), "after\n");
  assert.deepEqual(pi.messages, [{
    message: { customType: "stateful_coordination_warning", content: "overlapping work", display: true },
    options: { deliverAs: "nextTurn" },
  }]);

  const recorded = await hooks(hookLog);
  const pre = recorded.find(({ event, payload }) => event === "pre-tool-use" && payload.yolo);
  const post = recorded.find(({ event, payload }) => event === "post-tool-use" && payload.tool_call_id === "original-warn-call");
  assert.equal(pre.payload.tool_call_id, "original-warn-call");
  assert.equal(post.payload.tool_call_id, "original-warn-call");
  assert.equal(post.payload.is_error, false);
  assert.equal(post.payload.is_complete, true);
});

test("lazy write resume uses its granted subdirectory reservation without a redundant claim", async (t) => {
  const { extension, pi, hookLog, context: extensionContext, directory } = await loadExtension(t, {
    decision: {
      decision: "block",
      reason: "reservation pending",
      wait: { wait_id: "wait-subdir", reservation_id: "reservation-subdir" },
    },
    claimRequiresPath: true,
  });
  const cwd = join(directory, "packages", "client");
  await mkdir(join(cwd, "src"), { recursive: true });
  await writeFile(join(cwd, "src", "resume.js"), "before\n");
  const nestedContext = { ...extensionContext, cwd };
  const input = { path: "src/resume.js", content: "after\n" };

  await pi.handlers.get("tool_call")({
    toolCallId: "original-subdir-call",
    toolName: "write",
    reservationId: "reservation-subdir",
    input,
  }, nestedContext);
  assert.equal(extension.bindGrantedLazyReservation({
    kind: "reservation_granted",
    payload: { wait_id: "wait-subdir", reservation_id: "reservation-subdir" },
  }), true);
  const resumed = await pi.tools.get("lazy_write_resume").execute(
    "resume-subdir-tool-call",
    { operation_id: "wait-subdir" },
    undefined,
    undefined,
    nestedContext,
  );
  assert.equal(resumed.isError, false);

  assert.equal((await hooks(hookLog)).some(({ args }) => args?.[0] === "reservation" && args[1] === "claim"), false);
});

test("lazy write waits for its reservation grant and preserves it through completion", async (t) => {
  let reads = 0;
  let grantAcknowledged;
  const granted = new Promise((resolve) => { grantAcknowledged = resolve; });
  const { extension, pi, hookLog, context: extensionContext, directory } = await loadExtension(t, {
    decision: { decision: "block", reason: "reservation pending", wait: { wait_id: "wait-grant" } },
    fetchImpl: async (url) => {
      const pathname = new URL(url).pathname;
      if (pathname === "/v2/context/render") {
        return jsonResponse({ changed: false, workspace_version: 1, prompt_text: "" });
      }
      if (pathname === "/v2/notifications/poll") {
        grantAcknowledged();
        return jsonResponse({});
      }
      if (pathname === "/v2/notifications/stream") {
        return {
          ok: true,
          body: {
            getReader: () => ({
              read: async () => {
                if (reads++ === 0) {
                  const frame = "event: notification\n"
                    + "data: {\"notification_id\":\"wait-grant\",\"sequence\":1,\"kind\":\"reservation_granted\",\"payload\":{\"wait_id\":\"wait-grant\",\"reservation_id\":\"reservation-grant\"}}\n\n";
                  return { done: false, value: new TextEncoder().encode(frame) };
                }
                return new Promise(() => {});
              },
            }),
          },
        };
      }
      throw new Error(`unexpected URL ${url}`);
    },
    claimAlwaysFails: true,
  });
  await writeFile(join(directory, "granted.js"), "before\n");
  const input = { path: "granted.js", content: "after\n" };
  await pi.handlers.get("tool_call")({
    toolCallId: "original-grant-call",
    toolName: "write",
    input,
  }, extensionContext);

  const beforeGrant = await pi.tools.get("lazy_write_resume").execute(
    "resume-before-grant",
    { operation_id: "wait-grant" },
    undefined,
    undefined,
    extensionContext,
  );
  assert.equal(beforeGrant.isError, true);
  assert.match(beforeGrant.content[0].text, /not claimable/);
  assert.equal(extension.bindGrantedLazyReservation({
    kind: "reservation_granted",
    payload: { wait_id: "wait-grant", reservation_id: "wrong-reservation", relative_path: "other.js" },
  }), false);

  await pi.handlers.get("session_start")({ workspaceId: "workspace-1" }, extensionContext);
  await granted;
  const resumed = await pi.tools.get("lazy_write_resume").execute(
    "resume-after-grant",
    { operation_id: "wait-grant" },
    undefined,
    undefined,
    extensionContext,
  );
  assert.equal(resumed.isError, false);
  assert.equal(await readFile(join(directory, "granted.js"), "utf8"), "after\n");

  const recorded = await hooks(hookLog);
  assert.equal(recorded.some(({ args }) => args?.[0] === "reservation" && args[1] === "claim"), false);
  const pre = recorded.find(({ event, payload }) => event === "pre-tool-use" && payload.yolo);
  const post = recorded.find(({ event, payload }) => event === "post-tool-use" && payload.tool_call_id === "original-grant-call");
  assert.equal(pre.payload.reservation_id, "reservation-grant");
  assert.deepEqual(post.payload.result_metadata, {
    status: "applied",
    message: "lazy write applied",
    targets: ["granted.js"],
    wait_id: "wait-grant",
    reservation_id: "reservation-grant",
  });
});
