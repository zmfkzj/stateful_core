import assert from "node:assert/strict";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";

const [, , mode, extension] = process.argv;

if (mode === "run") {
  const { default: install } = await import(pathToFileURL(extension).href);
  const handlers = {};
  install({
    setLabel() {},
    on(name, handler) {
      handlers[name] = handler;
    },
  });
  if (process.env.STATEFUL_OMP_MODE === "startup") process.exit(0);

  const ctx = {
    cwd: "/workspace",
    sessionManager: {
      getSessionId: () => "session-1",
      getLeafId: () => "leaf-1",
    },
  };
  const call = { toolCallId: "call-1|unsafe", toolName: "write", input: { path: "note.txt", content: "after\n" } };
  await handlers.agent_start({}, ctx);
  await handlers.tool_call(call, ctx);
  await handlers.tool_result({ ...call, content: [{ type: "text", text: "wrote" }], isError: false }, ctx);
  if (process.env.STATEFUL_OMP_MODE === "shutdown") {
    await handlers.session_shutdown({ sessionId: "session-1" }, ctx);
  }
  process.exit(0);
}

const root = mkdtempSync(join(tmpdir(), "stateful-omp-terminal-"));

function scenario(name, responses) {
  const home = join(root, name);
  const state = join(root, `${name}-state.json`);
  const log = join(root, `${name}-log.jsonl`);
  const responseFile = join(root, `${name}-responses.json`);
  writeFileSync(responseFile, JSON.stringify(responses), { mode: 0o600 });
  const run = (runMode) => {
    const result = spawnSync(process.execPath, [process.argv[1], "run", extension], {
      encoding: "utf8",
      env: {
        ...process.env,
        HOME: join(root, "unused-home"),
        STATEFUL_HOME: home,
        STATEFUL_OMP_LOG: log,
        STATEFUL_OMP_STATE: state,
        STATEFUL_OMP_RESPONSES: responseFile,
        STATEFUL_OMP_MODE: runMode,
      },
    });
    assert.equal(result.status, 0, result.stderr);
  };
  const outbox = () => {
    const directory = join(home, "omp-terminal-outbox");
    return existsSync(directory) ? readdirSync(directory).filter((name) => name.endsWith(".json")) : [];
  };
  const terminalPayloads = () =>
    readFileSync(log, "utf8")
      .trim()
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line))
      .filter((entry) => entry.event === "post-tool-use")
      .map((entry) => entry.payload);
  return { home, outbox, run, terminalPayloads };
}

try {
  const accepted = scenario("accepted", [
    { decision: "allow", task_id: "task-1" },
    { decision: "allow", stateful: { write_attempt: { attempt_id: "attempt-1", permit_id: "permit-1" } } },
    { decision: "allow" },
  ]);
  accepted.run("send");
  assert.deepEqual(accepted.outbox(), []);
  assert.equal(accepted.terminalPayloads().length, 1);

  const replay = scenario("replay", [
    { decision: "allow", task_id: "task-1" },
    { decision: "allow", stateful: { write_attempt: { attempt_id: "attempt-1", permit_id: "permit-1" } } },
    { decision: "block" },
    { decision: "block" },
    { decision: "allow" },
  ]);
  replay.run("shutdown");
  const files = replay.outbox();
  assert.equal(files.length, 1);
  assert.match(files[0], /^[a-f0-9]{64}\.json$/);
  assert.equal(statSync(join(replay.home, "omp-terminal-outbox")).mode & 0o077, 0);
  assert.equal(statSync(join(replay.home, "omp-terminal-outbox", files[0])).mode & 0o077, 0);
  const pending = JSON.parse(readFileSync(join(replay.home, "omp-terminal-outbox", files[0]), "utf8"));
  assert.deepEqual(replay.terminalPayloads(), [pending, pending]);

  replay.run("startup");
  assert.deepEqual(replay.terminalPayloads(), [pending, pending, pending]);
  assert.deepEqual(replay.outbox(), []);

  const corrupt = scenario("corrupt", []);
  const corruptDirectory = join(corrupt.home, "omp-terminal-outbox");
  mkdirSync(corruptDirectory, { recursive: true, mode: 0o700 });
  const corruptName = "a".repeat(64) + ".json";
  writeFileSync(join(corruptDirectory, corruptName), "{", { mode: 0o600 });
  corrupt.run("startup");
  assert.deepEqual(corrupt.outbox(), []);
  assert.equal(
    readdirSync(corruptDirectory).filter((name) => name.startsWith(corruptName + ".bad-")).length,
    1,
  );
} finally {
  rmSync(root, { recursive: true, force: true });
}
