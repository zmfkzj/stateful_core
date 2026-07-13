# StatefulBench Docker runtime

StatefulBench runs the real-world shared-checkout corpus: ten issue-derived
tasks in each of ten pinned repositories. Every qualification and live run uses
the mandatory Docker image; there is no host-executed live runner.

The three arms are `sequential` (task agents in order), `parallel-off`
(concurrent task agents without Stateful), and `parallel-on` (concurrent task
agents with Stateful enforcement). The final reviewer starts after all task
agents finish. Do not compare runs with different images, models, thinking
settings, corpus revisions, task selections, or trial counts.

> **Current evidence:** the opt-in, credit-free Docker end-to-end gate has
> passed all three arms against a rebuilt `linux/arm64` image. Its fake agents
> prove Docker lifecycle, shared-checkout, and shared-HOME mechanics only. No
> model-backed full $10 \times 3 \times 3$ run has been performed, and no
> corpus-quality or arm-comparison result is claimed.

## Build and identify the Docker runtime

From the repository root, rebuild the currently tested and supported
`linux/arm64` image target:

```sh
IMAGE=statefulbench-realworld:linux-arm64
docker build --platform linux/arm64 \
  --file crates/stateful-bench/docker/statefulbench-realworld.Dockerfile \
  --tag "$IMAGE" .
docker image inspect --format '{{.Os}}/{{.Architecture}} {{.Id}} {{json .RepoDigests}}' "$IMAGE"
```

The runner requires `--docker-image` for both commands. Before qualification or
run it inspects exactly one Linux image, records its image ID, repository
digests, and native platform, and uses that platform for Docker containers.
Rebuild and requalify when any of those identity inputs, the manifest, corpus,
or archive pin changes.

The Docker **server**, not the host, decides the accepted platform. The published
`linux/arm64` image requires a Docker server that reports `linux/arm64`; the
runner rejects any image/server platform mismatch, so emulation is not accepted.

## Qualify before any live run

Use a cache outside the repository root. Qualification runs in Docker with the
repository mounted read-only at `/benchmark` and the cache mounted at `/cache`;
it downloads and checksum-verifies archives, then checks the base suite,
reference patches, integrated reference, evaluators, and upstream suite.

```sh
CACHE="$HOME/.cache/statefulbench-realworld"
python3 crates/stateful-bench/scripts/statefulbench_realworld.py qualify \
  --manifest datasets/statefulbench-realworld/manifest.json \
  --cache "$CACHE" \
  --docker-image "$IMAGE"
```

Use `--repo <repository-key>` repeatedly to qualify selected repositories.
Successful qualification writes
`<cache>/qualification/receipts/<repository>.json`. Each receipt binds the
manifest and corpus hashes, archive SHA-256 and commit, Docker image identity
(including image ID, repository digests, platform, and server platform), the
privately staged graded inputs (issue snapshot, task evaluators, task reference
patches, and integrated reference patch), and the exact six-tool map: Python,
OMP, Stateful SHA-256, Git, rustc, and Cargo. `run` rejects a missing or
mismatched receipt; changing any of these inputs requires requalification.
The reusable cache contains content-addressed archives, `pip-cache/`,
qualification artifacts, and receipts:

```text
<cache>/
  <archive-sha256>.tar.gz
  pip-cache/
  qualification/
    <repository>/artifacts/
    receipts/<repository>.json
```

Keep the cache to reuse verified archives and dependencies. Delete it only to
force archive redownload and requalification.

Receipts in this writable cache are operator workflow evidence: they make the
runner reject a locally missing or mismatched identity. They are not
tamper-resistant authorization against a person who controls the cache or the
checked-out code.

## Run the model-backed benchmark

The reporting run below selects all ten repositories, all three arms, and three
trials ($10 \times 3 \times 3$). It launches 990 model-backed OMP agents and
consumes substantial model credits; run it only on an explicit request. This
full run has **not** been performed.

```sh
OUT=".stateful_bench/statefulbench-realworld/$(date -u +%Y%m%d-%H%M%S)"
python3 crates/stateful-bench/scripts/statefulbench_realworld.py run \
  --manifest datasets/statefulbench-realworld/manifest.json \
  --cache "$CACHE" \
  --out "$OUT" \
  --trials 3 \
  --docker-image "$IMAGE"
```

Supported run options are `--repos` (a comma-separated repository list),
`--arms`, `--trials`, `--model`, `--thinking`, `--timeout-s`, and
`--docker-bin`. `--arms` defaults to `sequential,parallel-off,parallel-on`;
`--trials` defaults to 1. The qualification command accepts `--repo` repeatedly
and both commands accept `--docker-bin` (default `docker`).

For reproducible defaults, `run` uses
`openai-codex/gpt-5.6-terra`, `high` thinking, and a 900-second agent timeout.
Live execution requires the image-qualified `/usr/local/bin/omp` and
`/usr/local/bin/stateful` paths; do not substitute host or alternate binaries.

For each repository, arm, and trial, the harness starts one persistent arm
container. Every task agent and the final reviewer runs through `docker exec`
in that container, sharing `/workspace` and `HOME=/home/stateful`; no
per-agent HOME exists. `parallel-on` installs and enables Stateful in that
container and starts its enforcement server. The other arms do not enable
Stateful.

### Trusted-runtime boundary and credentials

This live-agent runtime is intentionally unrestricted: each arm container uses
`SYS_ADMIN`, unconfined seccomp/AppArmor and system-path policies, bridge
networking, OMP `--approval-mode yolo`, and
`STATEFUL_OMP_SANDBOX=off`. Run it only for a disposable, trusted corpus—not
for untrusted repositories, data, or credentials.

At arm setup, the harness selectively seeds only
`$HOME/.omp/profiles/stateful/agent/agent.db` rows whose provider is
`openai-codex` and whose credential type is `oauth`, when present. It creates a
temporary reduced database, copies that database into the container's OMP
profile, and deletes the host-side seed after setup. It does not mount or copy
the rest of host `HOME` or other host credentials. Without usable selected
OAuth credentials, model agents fail rather than receiving an alternate host
credential.

After the final reviewer, canonical evaluators and the pinned upstream suite
run in the same container. An arm clears only when there is no harness or
diagnostic error, every agent exits zero without timing out, evaluators and the
upstream suite pass, and the arm container is removed successfully.

## Results, cleanup, and diagnostics


Agent completion comes only through the trusted wrapper's Docker-client stdout
channel: exactly one JSON record with the agent PID, process-group ID, and exit
code. A missing, malformed, or nonzero Docker-client attestation forces arm
container cleanup and records agent exit code `-1`; the harness does not trust
agent-writable files for completion.

The runner removes each arm container with `docker rm -f`; a failed removal is
recorded and prevents clearance. It retains the host-mounted workspace and
runtime evidence:

```text
<out>/
  summary.json
  <repository>/<arm>/trial-<n>/
    results.json
    workspace/
    prompts/
    artifacts/
    runtime/
      logs/<agent>.stdout.log
      logs/<agent>.stderr.log
      diagnostics/{initialized,before-tasks,after-tasks,after-final,after-grading,before-remove}.json
```

`results.json` records agent exit/timeout/cleanup state, keyed
`evaluator_results` (`key` and `ok` for each task), evaluator and suite
outcomes, Docker runtime identity, and diagnostic classification.
`tasks_wall_time_s`, `final_wall_time_s`, and `arm_wall_time_s` measure only
task/final agent execution (excluding setup, diagnostics, grading, and
teardown). `end_to_end_wall_time_s` covers the full row; separate container
setup and teardown timings are retained. `summary.json` records all rows and
aggregates; both JSON files are atomically replaced.

Diagnostics are captured at the listed lifecycle phases. They prove the shared
container and HOME identities and summarize relative HOME files, databases,
locks, and process state. Snapshots reject absolute host paths and do not retain
secret contents. For a failed arm, read `results.json`, then the agent logs,
command artifacts, and diagnostics; preserve them with the matching receipt.

## Opt-in credit-free Docker gate

The Docker end-to-end test is skipped unless
`STATEFULBENCH_DOCKER_TEST_IMAGE` names an image. It uses a fake OMP executable,
does not require model credentials, and exercises all three arms, shared
`/workspace`, shared `/home/stateful`, grading, diagnostics, and cleanup:

```sh
STATEFULBENCH_DOCKER_TEST_IMAGE="$IMAGE" \
python3 -m unittest discover -s crates/stateful-bench/scripts/tests \
  -t crates/stateful-bench/scripts -p 'test_statefulbench_docker.py' -v
```

This is a runtime gate, not a model-backed benchmark result. Report live
results descriptively as run records and efficiency measurements; do not infer
behavioral quality, safety, causality, or statistical superiority from a gate,
single trial, or aggregate.
