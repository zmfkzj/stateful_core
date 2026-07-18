# StatefulBench parallel-on Initial Context Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Append Stateful session-start context without starting a competing OMP turn, then complete one cleared `requests` `parallel-on` trial on OMP 17.0.4.

**Architecture:** Keep normal context and reservation notifications unchanged. Parameterize only the context-delivery turn trigger, disable it for the initial `session_start` delivery, and preserve render/ack sequencing before rebuilding and evaluating the Docker runtime.

**Tech Stack:** JavaScript ES modules, Node test runner, Rust/Cargo, Docker, Python 3.14, OMP 17.0.4, StatefulBench real-world runner

## Global Constraints

- The product fix belongs to stateful_core's `crates/stateful-cli/assets/stateful-omp-extension.js`, not upstream OMP.
- Initial session context must be visible to the first model turn without starting a separate turn.
- SSE invalidation and reservation-ready notification turn behavior must remain unchanged.
- Keep the StatefulBench Docker runtime pinned to OMP 17.0.4.
- Do not change the Stateful server, benchmark runner, evaluators, corpus, or credential-copy logic.
- Model-backed execution scope is exactly `requests`, `parallel-on`, one trial, `openai-codex/gpt-5.6-terra`, thinking `high`, timeout `3600` seconds.
- If the row is uncleared, preserve evidence and stop; do not add another workaround.

---

### Task 1: Fix initial context delivery and verify the real arm

**Files:**
- Modify: `crates/stateful-cli/assets/stateful-omp-extension.js:388-430,1578-1601`
- Modify: `crates/stateful-cli/assets/stateful-omp-extension.test.mjs:152-171`
- Modify: `crates/stateful-bench/docker/statefulbench-realworld.Dockerfile:8`
- Create outside repository: `$HOME/.cache/statefulbench-realworld/cache/qualification/receipts/requests.json` through the harness
- Create outside repository: a fresh `$HOME/.cache/statefulbench-realworld/runs/requests-parallel-on-context-<UTC timestamp>/` result directory

**Interfaces:**
- Consumes: `deliverContext(pi, stream, targetVersion, triggerTurn)` where `triggerTurn` defaults to `true`.
- Produces: session-start custom message options `{ triggerTurn: false, deliverAs: "nextTurn" }`; all other `deliverContext` callers retain `triggerTurn: true`.

- [ ] **Step 1: Change the session-start expectation to the required contract**

In `stateful-omp-extension.test.mjs`, change the existing expected message options in `session start queues initial context for the next turn then acknowledges it` to:

```javascript
options: { triggerTurn: false, deliverAs: "nextTurn" },
```

Keep the assertions that `/v2/context/render` and `/v2/context/ack` are the first two requests.

- [ ] **Step 2: Run the focused test and verify red**

Run:

```bash
node --test \
  --test-name-pattern='session start queues initial context for the next turn then acknowledges it' \
  crates/stateful-cli/assets/stateful-omp-extension.test.mjs
```

Expected: FAIL because the current implementation sends `triggerTurn: true`.

- [ ] **Step 3: Implement the minimal stateful_core fix**

Change the function signature to:

```javascript
async function deliverContext(pi, stream, targetVersion, triggerTurn = true) {
```

Change its context message options to:

```javascript
{ triggerTurn, deliverAs: "nextTurn" }
```

Change only the session-start call to:

```javascript
if (!await deliverContext(pi, stream, undefined, false)) contextState.initialPending = true;
```

Do not change `deliverReservationNotification`, `deliverCoordinationWarning`, `flushContextDelivery`, or SSE processing.

- [ ] **Step 4: Run the focused test and verify green**

Run the Step 2 command again.

Expected: PASS; the initial message uses `triggerTurn: false`, and render/ack still occur.

- [ ] **Step 5: Run all OMP extension asset tests**

Run:

```bash
node --test crates/stateful-cli/assets/stateful-omp-extension.test.mjs
```

Expected: exit 0 with every extension test passing.

- [ ] **Step 6: Run the related stateful-cli tests**

Run:

```bash
cargo test -p stateful-cli
```

Expected: exit 0 with all stateful-cli unit and integration tests passing.

- [ ] **Step 7: Confirm and build the OMP 17.0.4 Docker runtime**

Ensure the Dockerfile contains:

```dockerfile
ARG OMP_VERSION=17.0.4
```

Then run:

```bash
docker build --platform linux/arm64 --pull \
  -f crates/stateful-bench/docker/statefulbench-realworld.Dockerfile \
  -t statefulbench-realworld:local .
docker image inspect statefulbench-realworld:local \
  --format '{{.Id}} {{.Os}}/{{.Architecture}} {{join .RepoDigests ","}}'
docker run --rm --platform linux/arm64 statefulbench-realworld:local omp --version
```

Expected: build exit 0, inspected platform `linux/arm64`, and `omp/17.0.4`.

- [ ] **Step 8: Run the credit-free Docker E2E**

Run from a Docker Desktop shared temporary root:

```bash
TMPDIR="$HOME/.cache" \
STATEFULBENCH_DOCKER_TEST_IMAGE=statefulbench-realworld:local \
python3 -m unittest discover \
  -s crates/stateful-bench/scripts/tests \
  -t crates/stateful-bench/scripts \
  -p 'test_statefulbench_docker.py' -v
```

Expected: exit 0 with all 47 tests passing, including the live fake-OMP three-arm E2E.

- [ ] **Step 9: Requalify requests against the rebuilt image**

Run:

```bash
python3 crates/stateful-bench/scripts/statefulbench_realworld.py qualify \
  --manifest datasets/statefulbench-realworld/manifest.json \
  --cache "$HOME/.cache/statefulbench-realworld/cache" \
  --docker-image statefulbench-realworld:local \
  --repo requests
```

Expected: exit 0 and a `qualified: true` receipt bound to the rebuilt `linux/arm64` image ID with OMP 17.0.4 provenance.

- [ ] **Step 10: Run one fresh requests parallel-on trial**

Run:

```bash
RUN_ID=$(date -u +%Y%m%dT%H%M%SZ)
OUT="$HOME/.cache/statefulbench-realworld/runs/requests-parallel-on-context-$RUN_ID"
python3 crates/stateful-bench/scripts/statefulbench_realworld.py run \
  --manifest datasets/statefulbench-realworld/manifest.json \
  --cache "$HOME/.cache/statefulbench-realworld/cache" \
  --out "$OUT" \
  --docker-image statefulbench-realworld:local \
  --repos requests \
  --arms parallel-on \
  --trials 1 \
  --model openai-codex/gpt-5.6-terra \
  --thinking high \
  --timeout-s 3600
```

Expected: exit 0 with one scheduled row and no `AgentBusyError`.

- [ ] **Step 11: Validate the locked row contract**

Read `$OUT/requests/parallel-on/trial-1/results.json` and verify:

```text
cleared == true
all(agent.exit_code == 0 for agent in agents)
post_suite_ok == true
evaluators_ok == true
upstream_suite_ok == true
container.removed == true
coordination_metrics.protocol_version == "stateful.v2"
qualification.image_id == runtime.image_id
runtime.platform == "linux/arm64"
```

Read `$OUT/summary.json` and verify the sole aggregate has `repo == "requests"`, `arm == "parallel-on"`, `row_count == 1`, and `cleared_count == 1`.

Expected: every condition is true. Any false condition means the task is incomplete.

- [ ] **Step 12: Commit and push only the verified change**

Run:

```bash
git add \
  crates/stateful-cli/assets/stateful-omp-extension.js \
  crates/stateful-cli/assets/stateful-omp-extension.test.mjs \
  crates/stateful-bench/docker/statefulbench-realworld.Dockerfile \
  docs/superpowers/plans/2026-07-18-statefulbench-parallel-on-context-injection.md
git commit -m "fix: avoid competing OMP context turn"
git push
```

Expected: only the extension, its regression test, Docker OMP pin, and this approved plan are committed after the model-backed row clears.
