# StatefulBench Real-World Corpus Design

**Status:** implementation and scoped validation complete  
**Approved:** 2026-07-11

## Goal

Replace the toy five-task `statefulbench-lite` workload with a reproducible real-world corpus drawn from original repositories represented in the DeNovo dataset. The corpus measures execution efficiency under the existing three-arm contract; it does not claim behavioral-quality, causal, safety, or statistical superiority.

## Repository set

The corpus contains these ten active Python repositories:

1. `psf/requests`
2. `python-jsonschema/jsonschema`
3. `pytest-dev/pytest-asyncio`
4. `pytest-dev/pytest-xdist`
5. `pallets/click`
6. `jschneier/django-storages`
7. `python-attrs/attrs`
8. `gorakhargosh/watchdog`
9. `python-pendulum/pendulum`
10. `authlib/authlib`

All ten occur as original repositories in the DeNovo public dataset. The DeNovo `sdispater/pendulum` URL redirects to the current canonical `python-pendulum/pendulum` repository.

## Source freeze

At corpus creation time, resolve each repository's default-branch HEAD once. Versioned provenance records:

- requested and canonical repository URLs;
- resolved commit SHA;
- source archive URL and SHA-256;
- snapshot timestamp;
- Python and package-manager versions;
- deterministic setup command; and
- upstream test command.

Source archives are not vendored. The runner downloads each archive once into a content-addressed local cache and rejects a checksum mismatch.

## Corpus shape

Each repository is one independent benchmark cell containing:

- five bug-fix tasks;
- five new-feature tasks; and
- one final integration review/fix task.

The full corpus therefore contains 100 coding tasks. Each repository cell runs in all three arms: `sequential`, `parallel-off`, and `parallel-on`. One trial launches 30 arms and 330 agent processes: $10\ \text{repositories} \times 3\ \text{arms} \times (10\ \text{task agents} + 1\ \text{final agent})$.

## Task sourcing and qualification

Tasks are curated from current open issue or roadmap families. A large issue may be split into independently testable tasks, and tightly related issues may be combined into one task. Every task retains source URLs and a hash of the frozen source text.

Each task must have:

- classification as `bug` or `feature`;
- an independent behavioral specification;
- at least three independent acceptance criteria;
- normal, boundary, and error-path evaluator coverage;
- a reference patch;
- a stable production symbol or named block anchor; and
- proof that its evaluator fails on the pinned base and passes with its reference patch.

A bug task must reproduce incorrect behavior on the pinned base. A feature task must demonstrate the specified behavior is absent on the pinned base. Documentation-only, typing-only, dependency-bump, formatting, and other mechanically trivial tasks are excluded.

## Overlap contract

Overlap is measured from reference patches at production symbol/block granularity.

- Tests, documentation, generated files, and lockfiles do not establish overlap.
- Two tasks overlap only when both reference patches modify the same qualified production symbol or the same named top-level/configuration block.
- Every task must overlap at least one other task; the per-repository overlap graph must have no isolated node.
- All ten task behaviors must be mutually compatible. A canonical integrated reference patch must pass every evaluator and the upstream suite together.

This usually produces five task pairs per repository, but denser valid graphs are allowed.

## Evaluator isolation

Task agents receive only their own task prompt and the pinned shared checkout. Reference patches and evaluator tests are not present in that checkout while task agents run.

After all ten task agents exit, the harness injects the evaluator tests. The final agent receives all ten specifications, runs the evaluator and upstream suites, and repairs incomplete behavior or merge damage. After the final agent exits, the harness independently reruns the same suites.

## Arm execution

Every repository/arm/trial owns a fresh checkout and fresh Stateful home created from the verified source archive. The runner rejects an existing `--out` directory or any scheduled `repo/arm/trial` directory before it reads the receipt or starts a container; never reuse output or row databases.

- `sequential`: ten independent task agents run in specification order.
- `parallel-off`: ten task agents run concurrently in one checkout without Stateful coordination.
- `parallel-on`: ten task agents run concurrently in one checkout and share one arm-local Stateful server started in `awareness` mode. It provides presence, freshness, handoff, rendered context, and advisory intent; enforcement is not the default benchmark arm.

The final agent runs only after all task-agent processes have been reaped. No checkout, container, or Stateful home is shared across repositories, arms, or trials. `parallel-on` is opt-in: omitted `--arms` means only `sequential,parallel-off`; every model-backed three-arm gate must spell `--arms sequential,parallel-off,parallel-on`.

## Completion and metrics

An arm is `cleared` only when all of these hold:

- all ten task agents and the final agent exit with code 0;
- no agent times out;
- no arm-level setup or server error is recorded; and
- the post-final evaluator and upstream suites pass.

The harness records:

- total tokens;
- total tool calls;
- arm wall time;
- aggregate task-agent wall time;
- final-agent wall time;
- cleared state;
- timeout, exit, setup, and server failure reasons; and
- per-repository and ten-repository aggregate results.

### Coordination metrics

Each cleared `parallel-on` row contains one locked, value-free `coordination_metrics` object with `protocol_version: "stateful.v2"`. `sequential` and `parallel-off` rows set it to `null`. An incomplete, malformed, non-monotonic, or post-admission diagnostic makes the on-arm row uncleared and its metrics `null`; an aggregate is present only when every scheduled on-arm trial is complete, never as a partial total.

The fixed object reports only:

- `journal`: `events`, `bytes_start`, `bytes_end`, `bytes_growth`, and allowlisted `by_event_type`;
- `presence`: `registered`, `expired`, `finalized`, and `peak_active`;
- `handoffs`: `explicit`, `fallback_stop`, `fallback_ttl`, and allowlisted `by_status`;
- `read_observations`: `started`, `stable`, `unstable`, `aborted`, and `invalidated`;
- `context`: `versions`, `renders`, `deliveries`, `acks`, `redeliveries`, `coalesced`, `prompt_utf8_bytes`, `prompt_unicode_scalars`, and `prompt_items`;
- `authorization`: allowlisted `warned_by_reason` and `denied_by_reason`;
- `write_safety`: `fence_conflicts`, `unknown_outcomes`, `same_path_overlaps`, and `cross_agent_overwrites`;
- `notifications`: allowlisted `by_kind`; and
- `waits`: allowlisted `by_final_status`, `grant_wait_time_s` (`count`, `total`, `mean`, `max`), and `unmeasured_grants`.

The object contains no payloads, identifiers, paths, resources, timestamps, free-form messages, raw database rows, raw server-log lines, or per-agent data. `mean` is `total / count` (or `null` at zero); summary means are weighted from totals and counts, and unmeasured grants are not zero-duration waits. `results.json` preserves the row's sanitized evidence and qualification identity; `summary.json` contains only sanitized report rows and locked aggregates.

These are observational coordination diagnostics, not causal proof. The credit-free Docker E2E and one cleared `requests` `parallel-on` trial are descriptive smoke/scoped evidence only. Do not make behavioral-quality, causal, safety, statistical, or superiority claims without an appropriately qualified multi-trial study.

## Qualification and launch gates

The Docker real-world runner is distinct from ProgramBench and DeNovoSWE. Its qualification receipt authorizes only this corpus/image identity.
The image pins OMP 17.0.4.

1. Build and inspect a `linux/arm64` image; the inspected image itself must report `linux/arm64`. A non-arm Docker daemon is private provenance, not an admission substitute.
2. Qualify the selected repository set against that exact image. A passing receipt at `CACHE/qualification/receipts/KEY.json` binds the manifest, corpus, archive, commit, staged graded inputs, image ID/platform/digests, and the six-tool map: Python, OMP, Stateful SHA, Git, Rustc, and Cargo. Rebuild, retag, or any bound-input change requires requalification.
3. Run only selected repositories with matching receipts, a new output directory, and explicit arms. `parallel-on` starts awareness; it is never implicit.
4. Preserve the fresh output directory and report only cleared rows. A full result is 10 repositories × 3 arms × 3 trials = 90 cleared rows; it has not yet been run.

```sh
IMAGE=statefulbench-realworld:local
BENCH_ROOT="$HOME/.cache/statefulbench-realworld"
CACHE="$BENCH_ROOT/cache"

docker build --platform linux/arm64 --pull \
  -f crates/stateful-bench/docker/statefulbench-realworld.Dockerfile \
  -t "$IMAGE" .
docker image inspect "$IMAGE" \
  --format '{{.Id}} {{.Os}}/{{.Architecture}} {{join .RepoDigests ","}}'

python3 crates/stateful-bench/scripts/statefulbench_realworld.py qualify \
  --manifest datasets/statefulbench-realworld/manifest.json \
  --cache "$CACHE" \
  --docker-image "$IMAGE" \
  --repo requests

RUN_ID=$(date -u +%Y%m%dT%H%M%SZ)
python3 crates/stateful-bench/scripts/statefulbench_realworld.py run \
  --manifest datasets/statefulbench-realworld/manifest.json \
  --cache "$CACHE" \
  --out "$BENCH_ROOT/runs/requests-$RUN_ID" \
  --docker-image "$IMAGE" \
  --repos requests \
  --arms sequential,parallel-off,parallel-on \
  --trials 1 \
  --model openai-codex/gpt-5.6-terra \
  --thinking high
```
