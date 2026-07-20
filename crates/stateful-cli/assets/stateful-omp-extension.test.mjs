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
  if (process.argv[2] === "reservation" && process.argv[3] === "declare") {
    process.stdout.write(JSON.stringify({ reservation_id: "reservation-auto" }));
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

function context(
  cwd = "/workspace",
  sessionId = "00000000-0000-4000-8000-000000000011",
  workspaceId = "workspace-1"
) {
  return {
    cwd,
    workspaceId,
    sessionManager: {
      getSessionId: () => sessionId,
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

test("state_exact_read returns every chunk before recording provenance", async (t) => {
  const { pi, hookLog, directory } = await loadExtension(t);
  const text = "first line\n" + "한".repeat(13000) + "\nlast line\n";
  await writeFile(join(directory, "large.py"), text);
  const exactRead = pi.tools.get("state_exact_read");
  const first = await exactRead.execute(
    "exact-read-1",
    { path: "large.py" },
    undefined,
    undefined,
    context(directory)
  );
  assert.equal(first.details.complete, false);
  assert.ok(first.content[0].text.startsWith("first line\n"));
  assert.ok(Buffer.byteLength(first.content[0].text, "utf8") < 24500);
  const started = await hooks(hookLog);
  assert.equal(started.at(-1).event, "pre-tool-use");
  assert.equal(started.at(-1).payload.tool_input.path, "large.py:raw");
  const stale = await exactRead.execute(
    "exact-read-stale",
    { path: "large.py", offset: first.details.next_offset, continuation_token: "invalid" },
    undefined,
    undefined,
    context(directory)
  );
  assert.equal(stale.isError, true);
  const second = await exactRead.execute(
    "exact-read-2",
    { path: "large.py", offset: first.details.next_offset, continuation_token: first.details.continuation_token },
    undefined,
    undefined,
    context(directory)
  );
  assert.equal(second.details.complete, true);
  assert.ok(second.content[0].text.includes("last line\n"));
  const recorded = await hooks(hookLog);
  assert.equal(recorded.at(-1).event, "post-tool-use");
  assert.equal(recorded.at(-1).payload.exact_read_candidate, true);
  assert.equal(recorded.at(-1).payload.tool_input.path, "large.py:raw");
  assert.equal(recorded.at(-1).payload.tool_call_id, started.at(-1).payload.tool_call_id);
  assert.equal(recorded.at(-1).payload.agent_id, started.at(-1).payload.agent_id);
});

test("state_exact_read resolves paths from a repository subdirectory", async (t) => {
  const { pi, hookLog, directory } = await loadExtension(t);
  const nested = join(directory, "crates", "example");
  await mkdir(nested, { recursive: true });
  await writeFile(join(nested, "source.py"), "contents\n");
  const result = await pi.tools.get("state_exact_read").execute(
    "nested-read",
    { path: "source.py" },
    undefined,
    undefined,
    context(nested)
  );
  assert.equal(result.isError, false);
  assert.equal(result.details.path, "crates/example/source.py");
  const recorded = await hooks(hookLog);
  assert.equal(recorded.at(-1).payload.cwd, directory);
  assert.equal(recorded.at(-1).payload.tool_input.path, "crates/example/source.py:raw");
});

test("state_exact_read keeps surrogate pairs in one chunk", async (t) => {
  const { pi, directory } = await loadExtension(t);
  const text = "x".repeat(11999) + "😀" + "y".repeat(2000);
  await writeFile(join(directory, "unicode.py"), text);
  const exactRead = pi.tools.get("state_exact_read");
  const first = await exactRead.execute(
    "unicode-1",
    { path: "unicode.py" },
    undefined,
    undefined,
    context(directory)
  );
  const offset = first.details.next_offset;
  assert.equal(first.content[0].text.slice(0, offset), text.slice(0, offset));
  assert.notEqual(text.charCodeAt(offset), 0xDE00);
  const second = await exactRead.execute(
    "unicode-2",
    { path: "unicode.py", offset, continuation_token: first.details.continuation_token },
    undefined,
    undefined,
    context(directory)
  );
  assert.equal(second.details.complete, true);
  assert.equal(second.content[0].text.slice(0, text.length - offset), text.slice(offset));
});

test("state_exact_read isolates concurrent agent progress", async (t) => {
  const { pi, directory } = await loadExtension(t);
  await writeFile(join(directory, "shared.py"), "x".repeat(13000));
  const exactRead = pi.tools.get("state_exact_read");
  const agentA = context(directory, "00000000-0000-4000-8000-000000000011");
  const agentB = context(directory, "00000000-0000-4000-8000-000000000012");
  const firstA = await exactRead.execute("a-1", { path: "shared.py" }, undefined, undefined, agentA);
  const firstB = await exactRead.execute("b-1", { path: "shared.py" }, undefined, undefined, agentB);
  const secondA = await exactRead.execute(
    "a-2",
    { path: "shared.py", offset: firstA.details.next_offset, continuation_token: firstA.details.continuation_token },
    undefined,
    undefined,
    agentA
  );
  const secondB = await exactRead.execute(
    "b-2",
    { path: "shared.py", offset: firstB.details.next_offset, continuation_token: firstB.details.continuation_token },
    undefined,
    undefined,
    agentB
  );
  assert.equal(secondA.details.complete, true);
  assert.equal(secondB.details.complete, true);
});

test("state_exact_read keeps one agent identity when OMP leaf ids change", async (t) => {
  const { pi, hookLog, directory } = await loadExtension(t);
  await writeFile(join(directory, "drift.py"), "x".repeat(13000));
  let leaf = 0;
  const changingLeafContext = context(directory);
  changingLeafContext.sessionManager.getLeafId = () => `leaf-${++leaf}`;
  const exactRead = pi.tools.get("state_exact_read");

  const first = await exactRead.execute("first", { path: "drift.py" }, undefined, undefined, changingLeafContext);
  const second = await exactRead.execute(
    "second",
    { path: "drift.py", offset: first.details.next_offset, continuation_token: first.details.continuation_token },
    undefined,
    undefined,
    changingLeafContext
  );

  assert.equal(second.details.complete, true);
  const recorded = await hooks(hookLog);
  assert.equal(recorded[0].payload.agent_id, recorded.at(-1).payload.agent_id);
  assert.equal(recorded[0].payload.agent_id, "omp-00000000-0000-4000-8000-000000000011");
});

test("state_exact_read rejects a continuation from another OMP session", async (t) => {
  const { pi, directory } = await loadExtension(t);
  await writeFile(join(directory, "drift.py"), "x".repeat(13000));
  const exactRead = pi.tools.get("state_exact_read");
  const first = await exactRead.execute(
    "drift-1",
    { path: "drift.py" },
    undefined,
    undefined,
    context(directory, "00000000-0000-4000-8000-000000000011")
  );
  const second = await exactRead.execute(
    "drift-2",
    { path: "drift.py", offset: first.details.next_offset, continuation_token: first.details.continuation_token },
    undefined,
    undefined,
    context(directory, "00000000-0000-4000-8000-000000000012")
  );

  assert.equal(second.isError, true);
  assert.match(second.content[0].text, /belongs to another identity/);
});
test("state_reconcile_ack rejects a read token from another OMP session", async (t) => {
  const { pi, directory } = await loadExtension(t);
  await writeFile(join(directory, "proof.py"), "proof");
  const proof = await pi.tools.get("state_exact_read").execute(
    "proof-read",
    { path: "proof.py" },
    undefined,
    undefined,
    context(directory, "00000000-0000-4000-8000-000000000011")
  );
  const reconciled = await pi.tools.get("state_reconcile_ack").execute(
    "proof-reconcile",
    {
      resources: ["proof.py"],
      files_reread: ["proof.py"],
      read_tokens: { "proof.py": proof.details.reconciliation_token },
      summary: "Adopt the completed exact reread.",
      decision: "adopt",
    },
    undefined,
    undefined,
    context(directory, "00000000-0000-4000-8000-000000000012")
  );

  assert.equal(reconciled.isError, true);
  assert.match(reconciled.content[0].text, /belongs to another identity/);
});

test("state_reconcile_ack exposes the native reconciliation path", async (t) => {
  const { pi, hookLog, directory } = await loadExtension(t);
  const result = await pi.tools.get("state_reconcile_ack").execute(
    "reconcile-call",
    {
      resources: ["src/lib.rs"],
      files_reread: ["src/lib.rs"],
      summary: "Adopt the integrated agent change.",
      decision: "adopt",
    },
    undefined,
    undefined,
    context(directory)
  );
  assert.equal(result.isError, false);
  assert.equal(result.details.reservation_id, "reservation-auto");
  const records = await hooks(hookLog);
  const declarationArgs = records.at(-2).args;
  assert.deepEqual(declarationArgs.slice(0, 2), ["reservation", "declare"]);
  assert.equal(declarationArgs.at(-1), "src/lib.rs");
  const args = records.at(-1).args;
  assert.deepEqual(args.slice(0, 2), ["reconcile", "ack"]);
  assert.equal(args[args.indexOf("--resource") + 1], "src/lib.rs");
  assert.equal(args[args.indexOf("--files-reread") + 1], "src/lib.rs");
  assert.equal(args[args.indexOf("--decision") + 1], "adopt");
  assert.equal(args[args.indexOf("--workspace-id") + 1], "workspace-1");
  assert.equal(args[args.indexOf("--reservation-id") + 1], "reservation-auto");
});

test("state_reconcile_ack forwards write intent recovery", async (t) => {
  const { pi, hookLog, directory } = await loadExtension(t);
  const result = await pi.tools.get("state_reconcile_ack").execute(
    "reconcile-intent",
    {
      resources: ["src/lib.rs"],
      files_reread: ["src/lib.rs"],
      summary: "Reconcile the unknown write.",
      decision: "reapply",
      intent_id: "intent-1",
    },
    undefined,
    undefined,
    context(directory)
  );
  assert.equal(result.isError, false);
  const records = await hooks(hookLog);
  assert.deepEqual(records.at(-2).args.slice(0, 2), ["reservation", "declare"]);
  const args = records.at(-1).args;
  assert.equal(args[args.indexOf("--intent-id") + 1], "intent-1");
  assert.equal(args[args.indexOf("--reservation-id") + 1], "reservation-auto");
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
  assert.equal(pi.messages.at(-1).message.content, "Recovered context");
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
  await pi.handlers.get("tool_result")({
    toolCallId: event.toolCallId,
    toolName: event.toolName,
    isError: event.isError,
    isComplete: event.isComplete,
    resultMetadata: event.resultMetadata,
  }, context());
  const recorded = await hooks(hookLog);
  const pre = recorded.find(({ event: name }) => name === "pre-tool-use").payload;
  const post = recorded.find(({ event: name }) => name === "post-tool-use").payload;
  assert.equal(pre.tool_call_id, "call-correlation");
  assert.equal(post.tool_call_id, "call-correlation");
  assert.equal(post.is_error, false);
  assert.equal(post.is_complete, true);
  assert.equal(post.exact_read_candidate, true);
  assert.deepEqual(post.tool_input, { path: "src/lib.rs:raw" });
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
