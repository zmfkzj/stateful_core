# StatefulBench parallel-on OMP Upgrade Implementation Plan
> **상태: Superseded.** OMP 17.0.4 단독 상향은 `AgentBusyError`를 해결하지 못했습니다. 이 plan은 [2026-07-18-statefulbench-parallel-on-context-injection.md](2026-07-18-statefulbench-parallel-on-context-injection.md)로 대체되었습니다.


> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pin the StatefulBench real-world Docker runtime to OMP 17.0.4 and complete one cleared `requests` `parallel-on` trial.

**Architecture:** Change only the Docker image's pinned OMP version. Rebuild and qualify the resulting immutable `linux/arm64` image, then run one model-backed `parallel-on` row and validate its locked result fields.

**Tech Stack:** Docker, Python 3.14, OMP 17.0.4, StatefulBench real-world runner, unittest

## Global Constraints

- Modify only `OMP_VERSION`; do not change the Stateful extension, runner, evaluators, corpus, or credential-copy logic.
- The tested image must inspect as `linux/arm64` and run OMP 17.0.4.
- Qualification and execution must use the same inspected image identity.
- Model-backed execution scope is exactly `requests`, `parallel-on`, one trial, `openai-codex/gpt-5.6-terra`, thinking `high`, timeout `3600` seconds.
- If the row is uncleared, stop and report the evidence; do not add a benchmark-specific fallback.

---

### Task 1: Upgrade and verify the Docker runtime

**Files:**
- Modify: `crates/stateful-bench/docker/statefulbench-realworld.Dockerfile:8`
- Test: `crates/stateful-bench/scripts/tests/test_statefulbench_docker.py`

**Interfaces:**
- Consumes: Docker build argument `OMP_VERSION` used by `bun install -g "@oh-my-pi/pi-coding-agent@${OMP_VERSION}"`.
- Produces: inspected image tag `statefulbench-realworld:local` containing OMP 17.0.4 on `linux/arm64`.

- [ ] **Step 1: Change the pinned runtime version**

Replace:

```dockerfile
ARG OMP_VERSION=16.4.2
```

with:

```dockerfile
ARG OMP_VERSION=17.0.4
```

- [ ] **Step 2: Build the exact runtime image**

Run:

```bash
docker build --platform linux/arm64 --pull \
  -f crates/stateful-bench/docker/statefulbench-realworld.Dockerfile \
  -t statefulbench-realworld:local .
```

Expected: exit 0; the Dockerfile verification layer prints `omp/17.0.4`.

- [ ] **Step 3: Inspect image identity and OMP version**

Run:

```bash
docker image inspect statefulbench-realworld:local \
  --format '{{.Id}} {{.Os}}/{{.Architecture}} {{join .RepoDigests ","}}'
docker run --rm --platform linux/arm64 statefulbench-realworld:local omp --version
```

Expected: the first command reports `linux/arm64`; the second reports `omp/17.0.4`.

- [ ] **Step 4: Run the credit-free Docker end-to-end suite**

Run:

```bash
STATEFULBENCH_DOCKER_TEST_IMAGE=statefulbench-realworld:local \
python3 -m unittest discover \
  -s crates/stateful-bench/scripts/tests \
  -t crates/stateful-bench/scripts \
  -p 'test_statefulbench_docker.py' -v
```

Expected: exit 0 with every Docker end-to-end test passing, including all-arm shared-HOME, grading, diagnostics, and cleanup coverage.


---

### Task 2: Qualify and evaluate one requests parallel-on row

**Files:**
- Create outside repository: `$HOME/.cache/statefulbench-realworld/cache/qualification/receipts/requests.json` through the harness
- Create outside repository: a fresh `$HOME/.cache/statefulbench-realworld/runs/requests-parallel-on-<UTC timestamp>/` result directory

**Interfaces:**
- Consumes: inspected `statefulbench-realworld:local` identity from Task 1 and host `openai-codex` OAuth credentials.
- Produces: one immutable `requests/parallel-on/trial-1/results.json` and a sanitized `summary.json`.

- [ ] **Step 1: Requalify requests against the rebuilt image**

Run:

```bash
python3 crates/stateful-bench/scripts/statefulbench_realworld.py qualify \
  --manifest datasets/statefulbench-realworld/manifest.json \
  --cache "$HOME/.cache/statefulbench-realworld/cache" \
  --docker-image statefulbench-realworld:local \
  --repo requests
```

Expected: exit 0 and `requests.json` records `qualified: true`, `platform: linux/arm64`, and the rebuilt image ID.

- [ ] **Step 2: Run one fresh model-backed parallel-on trial**

Run:

```bash
RUN_ID=$(date -u +%Y%m%dT%H%M%SZ)
OUT="$HOME/.cache/statefulbench-realworld/runs/requests-parallel-on-$RUN_ID"
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

- [ ] **Step 3: Validate the locked row contract**

Read `$OUT/requests/parallel-on/trial-1/results.json` and verify all of the following exact conditions:

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

Expected: every condition is true. Any false condition means the goal is not complete.

- [ ] **Step 4: Commit and push the verified runtime pin**

Run:

```bash
git add crates/stateful-bench/docker/statefulbench-realworld.Dockerfile
git commit -m "fix: upgrade StatefulBench OMP runtime"
git push
```

Expected: only the Dockerfile is included in the commit and the current branch is pushed successfully after the model-backed row clears.
