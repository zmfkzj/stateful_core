# Stateful Coordination Task-Graph Benchmark — Implementation Spec

Status: Proposed  
Date: 2026-07-10

> **Scope.** This document specifies a new graph-execution benchmark extension. It does not describe an implemented runner. Unless a paragraph is explicitly labelled **Current**, every requirement below is **Proposed**. Existing normal DeNovoSWE/ProgramBench runs, condition matrices, forced-overlap evidence, and checked reports remain unchanged and MUST NOT be relabelled as this benchmark.

## Objective and causal contrasts

**Proposed.** Measure whether Stateful improves a fixed public task graph without degrading hidden behavioral correctness. One benchmark cell is evaluated in three arms:

- `sequential`: one persistent agent executes ready graph nodes in order.
- `parallel-off`: fixed harness-owned workers execute ready nodes concurrently without a usable Stateful runtime.
- `parallel-on`: the identical parallel protocol executes with the full Stateful treatment: enforcement server, installed OMP profile/tools, and mediated runtime.

The benchmark reports two paired contrasts:

1. `parallel-off - sequential` is the **parallel-regime contrast**. It replaces one persistent serial agent with the defined N-worker protocol. Context partitioning, per-node turn caps, failure propagation, and runtime topology also change; it is **not** a concurrency-only estimate.
2. `parallel-on - parallel-off` is the sole causal estimate of the **full Stateful treatment**. It is not an estimate of the policy engine in isolation.

`parallel-on - sequential` is secondary context only. No weighted composite may replace the two primary contrasts.

**Proposed instance definition.** A benchmark instance is one base workspace, one fixed public Task DAG, one private behavioral oracle, and one final Eval Commit. A hidden target commit is optional reference material for test construction, never a patch-equality objective. ProgramBench may instead use its supplied reference executable as a binary oracle without a target source commit.

The benchmark scores behavioral correctness with hidden tests and separately reports wall-clock time, aggregate agent activity, model use, and attributable coordination overhead. It never asserts an unobserved counterfactual “prevented collision”; it reports only observed blocked collision risks and verified recoveries.

## Current implementation boundary

**Current — condition matrices, not graph mode.** `DeNovoCondition` and `ProgramBenchCondition` each expose the current `(stateful, subagent)` axes. Their stable report IDs are `stateful-off_subagent-off`, `stateful-on_subagent-off`, `stateful-off_subagent-on`, and `stateful-on_subagent-on`; the CLI accepts equivalent input such as `stateful:on,subagent:on`. The existing paired comparison uses the two `subagent:on` conditions.

**Current — subagents do not establish an N-worker protocol.** In DeNovo, `subagent:on` adds the literal `orchestrate` instruction and enables/audits native multi-agent support; ProgramBench likewise appends `orchestrate` and its Codex path enables multi-agent support. Neither current adapter establishes an exact worker count, assigned node, dependency order, or completion protocol. The proposed graph mode disables native model-controlled spawning in every arm and makes workers harness-owned.

**Current — concurrency has different meanings.** DeNovo `--max-concurrent` / `max_concurrent_limit` semaphore-limits independent benchmark instances; it does not schedule graph nodes within an instance. `run_programbench_matrix_with_instances` performs serial agent inference across conditions and instances. ProgramBench `eval --workers` is evaluator-only, not graph-worker concurrency.

**Current — ProgramBench workspace topology is not a shared checkout.** Every current adapter invocation begins from a fresh host airlock copied from target `/workspace`; host CLI runs directly there. Only the optional Docker OMP path copies that airlock into a separate agent container and copies it back after the agent finishes. Neither topology provides simultaneous shared-checkout visibility; Docker copy-back has no concurrent merge semantics. Proposed graph-mode ProgramBench replaces both with one initialized canonical airlock bind-mounted read-write into all node containers.

**Current — reusable overlap plumbing has validity gaps.** `run_pair_inner` demonstrates one root, concurrent children, and one combined patch in a shared workspace. Its checked OMP wrapper parses workspace/session values but does not use them to prove a join to the runner server, and it does not map a logical assignment to the OMP-derived Stateful agent identity. It is reusable topology evidence, not a valid task-graph benchmark.

**Current — telemetry is incomplete for the proposed claim.** DeNovo `summarize_orchestration_events` computes `true_collisions_prevented`, `self_inflicted_denials`, and `scope_overlap_warnings`, but `write_orchestration_trace` omits all three from the compact result summary that reaches Rust aggregation. ProgramBench has no lifecycle/coordination trace, normalized tool/write/retry stream, or semantic duplicate-investigation/merge metric. Repair the DeNovo compact path and add its end-to-end propagation test before using its fields; add the missing ProgramBench instrumentation before reporting equivalent metrics. `true_collisions_prevented` remains a legacy event-derived field, not causal proof.

**Current — audit export is not lossless.** `/v1/events` is latest-first, capped at 1,000, and filters with whole-second `created_at > since`; its audit export has no ordered fixed-watermark cursor. Notification replay sequence is a separate per-agent delivery mechanism and is not an audit-event cursor. Stored lifecycle names are `AgentRegistered`, `AgentHeartbeat`, and `ActivityFinalized`.

**Current evidence boundary.** Ordinary DeNovo instances rarely create enough same-file overlap to establish coordination efficacy. The checked-in forced-overlap harness has no-state, awareness, and enforcement plumbing, but its three-arm record has equal observed safety outcomes and omits duplicated-investigation time and usable token/tool telemetry. Historical raw denials are not coordination value: the 2026-07-04 analysis found only four blocker-backed `active_claim_conflict` events over 30 stateful-on rollouts and attributes much friction to runtime/self-policy failures.

**Source anchors.** Current behavior above is grounded in `crates/stateful-bench/src/denovo.rs` (`DeNovoCondition`), `crates/stateful-bench/src/programbench.rs` (`ProgramBenchCondition`, `run_programbench_matrix_with_instances`), `crates/stateful-bench/src/lib.rs` (`run_pair_inner`), the DeNovo and ProgramBench adapter scripts, `crates/stateful-server/src/lib.rs` (`/v1/events`, `/v1/runtime/identity`), `crates/stateful-store/src/lib.rs`, and the existing benchmark guides and collision analysis. Later sections define new requirements; they MUST NOT be read as claims about those current paths.

## Instance and graph contract

**Proposed public manifest location.** Store frozen public manifests at `datasets/coordination/task-graphs/<benchmark>/<instance_id>.json`. Every manifest has this exact public shape:

```json
{
  "schema_version": "stateful.task-graph.v1",
  "graph_id": "<benchmark>/<instance_id>/v1",
  "benchmark": "denovo",
  "instance_id": "<public instance id>",
  "source_files": ["README.md"],
  "source_digest": "sha256:<digest of agent-visible requirements>",
  "nodes": [
    {
      "id": "T01",
      "title": "<short public requirement>",
      "instruction": "<behavior to implement>",
      "acceptance": ["<observable public behavior>"],
      "depends_on": [],
      "public_refs": ["README.md#<public section>"],
      "resources": [
        {"kind": "artifact", "id": "src/example.py", "access": "write"}
      ]
    }
  ]
}
```

`benchmark` is exactly `denovo` or `programbench`. Node IDs are unique `T01`-style IDs. `source_files` is a nonempty sorted list of explicit POSIX paths, never globs. Compute `source_digest` by sorting those files and hashing, in order, `path UTF-8 + NUL + decimal byte length + NUL + raw bytes`; do not normalize contents. A ProgramBench manifest may list its standard agent-visible `executable`, but contains no hidden base/reference SHA, target source/diff/derived source path, golden test data, test name, evaluator result, or hidden metadata.

Each node has 1–3 public `acceptance` items, at least one `public_refs` entry, and a nonempty `resources` array. A resource is exactly `{kind: "artifact" | "contract", id: "<resource>", access: "read" | "write"}`:

- For `artifact`, `id` MUST be a normalized checkout-relative path, or one member of a finite public path set, appearing in cited public requirements.
- For `contract`, `id` is a public logical API or schema identifier.
- IDs MUST NOT contain NUL or newline. Authors and reviewers MUST agree resources before graph freeze.

Freeze resource declarations into `graph_sha256` and `resource_digest`. `graph_sha256` is SHA-256 of the exact frozen public-manifest bytes. `resource_digest` hashes sorted UTF-8 records `task_id + NUL + kind + NUL + id + NUL + access + LF`. The private manifest has its own `evaluator_digest`, SHA-256 of its exact frozen bytes.

**Proposed private evaluator boundary.** Evaluator-only fields are `base.repository`, `base.revision`, `reference.kind` (`target_commit | binary_oracle`), `reference.id`, `coordination_stratum`, `task_tests`, `integration_tests`, `regression_tests`, and `evaluator_digest`. They MUST NOT be mounted into an agent workspace, prompt, resource declaration, or stratum instruction.

**Proposed graph validation.** Before freeze, a graph MUST have 4–8 nodes, be acyclic, have maximum ready width 2–4, contain at least one fork and one join, and have a nonempty private hidden-test set for every node. Each ready node MUST be implementable from public context plus predecessor outputs without subsuming another node. Two benchmark-author reviewers approve the graph and its stratum before evaluator mapping. The deterministic scheduler tie-break is ascending node ID.

## Confirmatory dataset

**Proposed candidate frame.** Publish `datasets/coordination/task-graphs/<benchmark>/candidate-frame.json` before any run. For every candidate it records the instance and graph IDs; graph, source, resource, and evaluator digests; public structural eligibility/reason; booleans `private_mapping_complete` and `private_evaluator_certified`; and public-resource-derived stratum. It MUST NOT contain hidden test names/content/results.

Selection has three mandatory gates: structural eligibility, complete private mapping, and evaluator certification. Only certification pass/fail may be consulted; neither score magnitude nor agent outcome may influence selection. Classify candidates by incomparable pairs:

- `hotspot`: both nodes write the same public normalized `artifact`;
- `shared_contract`: otherwise, a pair shares a `contract` and at least one writes;
- `independent`: no pair shares any resource with a writer.

Reject mixed cases, including artifact read/write pairs lacking two writers. For each benchmark, sort candidate IDs, initialize a new NumPy `Generator(PCG64(42))`, and call `choice(..., replace=False)` in stratum order `independent`, `shared_contract`, `hotspot` with quotas `(3, 4, 3)`. Fewer eligible candidates in any quota makes the release incomplete; no hand selection is allowed.

**Proposed freeze rule.** Freeze selected graph bytes, resource declarations, selection output, prompts, and hashes before inference. No hidden target source/diff/golden/test artifact may shape agent-visible instructions, resources, or strata. ProgramBench's executable remains standard agent input. Private evaluator certification can exclude an invalid candidate only; planning quality is not a score.

**Proposed standard run profile.** Build `crates/stateful-bench/docker/denovo-omp-agent.Dockerfile` at the repository root as `stateful-task-graph-omp:<source-revision>` and record immutable image ID/digest. Both adapters use that one agent image. Pin `omp-cli`, model `openai-codex/gpt-5.6-terra`, reasoning `high`, workers `4`, aggregate turns `500`, and arm deadline `7,200` seconds.

- DeNovo uses prompt `v2`, temperature `1`, context `256000`, `eval-iters 1`, model-provider access, and ordinary public dependency/research egress through the configured target-source deny proxy.
- ProgramBench uses target image tag `task_cleanroom_v6`. Its target container is `--network none`; OMP egress is allowlisted only to recorded model-provider endpoints and the ephemeral Stateful endpoint, never task-source/package internet.
- Pin immutable ProgramBench package, task-data, and test-blob revisions and official evaluation `workers=4`, `branch-workers=2`, `docker-cpus=8`.
- Record every value, endpoint allowlist, default resolved by either adapter, image platform/ID/digest, exact OMP/Stateful/Python/NumPy versions and binary digests, and provider model ID/revision/fingerprint per response when exposed in `run-manifest.json`. A reported model revision change inside a matrix invalidates its affected logical attempt set.

**Proposed confirmatory host rule.** Confirmatory execution requires a Linux `amd64` Docker host. Build and run both agent and target images for `linux/amd64`; record host OS/architecture, Docker version, image platform, ID, and digest. A macOS/arm64 host-only result or cross-platform-emulated result is non-confirmatory.

**Proposed cell isolation.** For every `(benchmark, graph_id, trial_id, arm, attempt_id)`, materialize a fresh cell-local canonical checkout/airlock, OMP homes, containers, run ID, and—only for `parallel-on`—server home/event database. Before `arm_init`, require recorded base HEAD, clean tracked and untracked trees, and matching base/source digests. Only workers inside one cell share its checkout. Copy finalized artifacts out, then tear down the full cell; never reuse sessions, trees, runtime homes, or databases.

Run one graph arm at a time with no unrelated benchmark jobs. Keep graph order fixed and counterbalance arm order: trial 1 is `sequential, parallel-off, parallel-on`; trial 2 is `parallel-off, parallel-on, sequential`; trial 3 is `parallel-on, sequential, parallel-off`. Retried attempt sets preserve that trial order.

## Execution protocol

**Proposed graph-mode CLI.** Add repeated `--execution-arm` values `sequential`, `parallel-off`, and `parallel-on`, plus `--task-graph`, `--graph-workers 4`, `--total-model-turns 500`, `--trial-id`, and `--timeout-seconds 7200`. Native subagent spawning is disabled in every arm. All workers are harness-owned.

**Proposed harness state machine.** The controller is the sole scheduler and uses `pending | ready | started | finished | model_failed | blocked` in all arms. `finished` means the assigned OMP request completed; it does not imply hidden behavior passed and releases newly ready descendants. `model_failed` blocks undispatched descendants while independent ready nodes may continue if their controller/session remains usable. Global sequential budget exhaustion or an unusable persistent session blocks every unfinished node. Hidden tests never feed back into scheduling. There is no automatic task retry.

For every started task, the controller writes exactly one `stateful.bench.node-terminal.v1` envelope:

```json
{
  "run_id": "<run>",
  "trial_id": "<trial>",
  "attempt_id": "<attempt>",
  "graph_id": "<graph>",
  "arm": "parallel-on",
  "task_id": "T01",
  "agent_id": "<harness agent>",
  "runtime_agent_id": "<optional actual OMP identity>",
  "status": "finished",
  "origin": "model",
  "reason_code": "<reason>",
  "session_usable": true,
  "omp_exit_code": 0,
  "completion_sha256": "<optional completion>",
  "turns_used": 0,
  "monotonic_ns": 0
}
```

`session_usable` is required for sequential failures. Exit `0` plus nonempty completion within cap yields `finished`; budget/context/model timeout or empty completion yields `model_failed`; provider/container/harness origins invalidate the cell. The controller validates correlation and the cap ledger before a transition. A live controller that fails to emit one matching terminal envelope makes the cell harness-invalid.

**Proposed instruction equivalence.** Every arm receives byte-identical node title, instruction, public references, and resources. Sequential receives common public context once and then one ready node per prompt. Each parallel worker receives the same common context plus one `assigned_task_id`, may implement only that node, and may not spawn children. Stateful's declared tool/runtime context is the only prompt/runtime-context difference between the two parallel arms. There is no model coordinator.

**Proposed `sequential` arm.** Use one persistent OMP session, the same Stateful-off mask/preflight as `parallel-off`, and no native subagents. Send frozen common context once; issue one ready node at a time in ascending topological order; write its terminal state before the next prompt. Aggregate turn accounting spans the session. Never replace an unusable session or a failed task with a fresh agent; apply the blocking rule.

**Proposed `parallel-off` arm.** Use the same image, provider/network policy, shared `/workspace` bind mount, isolated per-task child home, scheduler, per-node budget, and wrapper as `parallel-on`, with at most four concurrent OMP containers. Do not install a Stateful profile/runtime. Mask the image's Stateful executable; deny every Stateful process/network attempt; preflight that no `STATEFUL_*` runtime variable/file, URL/token, extension, server, event stream, or executable path is usable.

**Proposed `parallel-on` arm.** Use the identical scheduler policy and bind every node container read-write to the same host checkout at `/workspace`, with one child home per task. ProgramBench graph mode MUST NOT use per-agent copy-in/copy-back. Start one ephemeral enforcement server with `--workspace-id <run_id>-<graph_id>` on dedicated local Docker transport and derive the container URL with `docker_host_url`.

Before real workers, a disposable container with identical mounts/network MUST run the same OMP installation and:

```text
stateful server join <container-url> --token <token> --workspace-id <id> --enable-repo --allow-plain-http
```

It then fetches `/v1/runtime/identity` through joined discovery. This is a proposed trusted-Docker transport proof, not the current join behavior. Plain HTTP opt-in is limited to that fresh ephemeral trusted Docker path. Generate a new token, redact command/log output, and fail the preflight rather than falling back to private workspaces.

For every real worker, set `HOME` and `STATEFUL_HOME` to its same bind-mounted child-home path; run `stateful install --agent omp --yes --binary <stateful-binary>` and join from `/workspace`. The disposable preflight initializes shared `.stateful/config.yml` once. Record its hash, serialize all child joins in ascending task ID, and require the hash unchanged. Finalization always excludes `.stateful/`. Remove `STATEFUL_SERVER_URL` and `STATEFUL_SERVER_TOKEN` before OMP launch, then set `PI_CODING_AGENT_DIR=<child-home>/.omp/profiles/stateful/agent`. Any post-join runtime override is a validity failure because it replaces the joined workspace ID with `unknown`.

**Proposed task/identity handshake.** Graph-mode Docker launch explicitly allowlists `STATEFUL_BENCH_TASK_ID`, `STATEFUL_BENCH_ATTEMPT_ID`, and `STATEFUL_BENCH_IDENTITY_PATH`; translate the last to a writable container-visible file in the mounted child home. At session start, the Stateful OMP extension atomically writes:

```json
{"task_id":"T01","attempt_id":"<attempt>","session_id":"<session>","leaf_id":"<leaf>","actual_agent_id":"omp-<session>-<leaf>","workspace_id":"<workspace>"}
```

using `ctx.sessionManager`. Freeze `stateful/agent-map.json`. It MUST form a bijection between started task/attempt pairs and actual OMP identities; blocked and unstarted tasks have no identity.

**Proposed lifecycle evidence.** Independently of tools, emit one `AgentRegistered`, one immediate `AgentHeartbeat` within one second, periodic heartbeats every 15 seconds with maximum observed active gap 20 seconds, and one `ActivityFinalized` for every mapped agent. Each mapped identity and each `/v1/runtime/identity` response MUST match server pid, `protocol_version: "stateful.v1"`, required capabilities, common workspace evidence, and `coordination_mode: "enforcement"`. Persist only scrubbed evidence. The immediate/periodic cadence, task mapping, and workspace proof are new graph-mode requirements; current runtime identity does not expose all of them.

## Compute budget and finalization

**Proposed hard compute gate.** Every arm has exactly 500 aggregate model turns. Sequential has all 500. For N parallel nodes, each cap is `floor(500/N)` and the first `500 mod N` ascending IDs receive one extra; caps sum to 500 and unused turns are never transferred. Add a graph-mode OMP `--max-model-turns <n>` gate before efficacy runs; prompt text is not enforcement.

One turn is one logical OMP agent-loop invocation to the provider capable of producing an assistant response. Transport retries of that invocation do not add turns but are recorded. Tool calls and response chunks are not turns. Emit indexed `model_turn_started` and `model_turn_completed`; refuse invocation N+1; emit `budget_exhausted` as a scored model-terminal outcome. Audit total/cached/uncached input, output, reasoning tokens, turns, transport retries, and tool calls. Tokens are outcomes, never post-hoc matching variables. Missing/nonmonotonic ledgers or observed turns above cap make the rollout runtime-invalid.

**Proposed clock.** Let `H = 7_200_000 ms`. After common canonical-checkout/image preparation, record `arm_init` immediately before every arm-specific action: Stateful server/install/join, node-container launch, agent work, waits, and finalization. Use one monotonic clock and enforce H as the whole-arm deadline. Hidden evaluation and common preparation are excluded and reported separately.

H expiry is a scored right-censored `origin: deadline` outcome. At H, close the model gate, deny further workspace mutation, mark started nodes `model_failed`, mark undispatched nodes `blocked` with `reason_code: arm_deadline`, and record H. Give OMP a fixed ten-second teardown grace outside the endpoint to emit `ActivityFinalized`, then force-kill leftovers. Inability to stop, freeze, or finalize each mapped identity is `origin: harness` and runtime-invalid. Only after the lifecycle gate may the runner drain the fixed event snapshot and capture a frozen partial tree.

**Proposed deterministic finalizer.** After all nodes are terminal and before H, verify unchanged base HEAD, reject agent commits, apply exclusions, stage allowed output once, and create one Eval Commit whose only parent is recorded base. On scored H expiry, run the same finalizer after freeze only to capture the partial tree for correctness evaluation; its late commit can never be timely resolved. Record parent/commit/tree/patch hashes, staged paths, monotonic start/end, and validation. Failure to create either required commit is harness-invalid.

Use identity `Stateful Bench <stateful-bench@local>`, message `eval: <graph_id> <arm> <trial_id>`, and author/committer time `base_commit_time + 1 second`. Initialize ProgramBench's canonical airlock Git base in every arm. Agents never branch, commit, or merge; shared-checkout patch conflict and merge failure are `not_applicable`.

## Evaluation contract

**Proposed behavioral equivalence.** DeNovo uses its fresh evaluator: reset base, apply the Eval Commit patch, remove agent-authored tests/scripts, inject golden tests/fixtures, install, and run hidden pytest. ProgramBench archives the finalized Eval Commit tree, runs official `eval`, `info`, and `submit package`, and treats `_stats/score.json` as native-score source of truth.

Before any agent copy, ProgramBench records upstream `instance_id`, `repository`, `commit`, immutable target-image digest, deterministic sorted-path hash of target `/workspace`, and exact supplied executable hash. Define private `reference.id` as:

```text
sha256(instance_id + NUL + repository + NUL + commit + NUL + workspace_digest + NUL + executable_digest)
```

Pin immutable package, task-data, and test-blob revisions; never floating `main`. Do not expose provenance/test metadata or results to agents. The ProgramBench target-workspace digest reuses source-digest path/length/raw-byte encoding over every sorted pre-agent `/workspace` file; the executable digest hashes exact supplied bytes.

**Proposed private oracle mapping.** The private manifest assigns every hidden test to exactly one nonempty node set `Q_i`, `integration_tests`, or `regression_tests`; there is no double counting. Let `V_g` be every frozen node and `Q_g` be the union of those three buckets. Missing evaluator output or inability to produce pass/fail is invalid; ordinary candidate failures remain scored and never shrink a denominator.

Before selection, certify the private evaluator in three fresh deterministic runs: the reference passes every `Q_g` test each time; the base passes packaging through the same test exclusions and has at least one stable non-pass in every task bucket; per-test results agree across runs. Integration/regression tests may pass on base. A flake or reference failure makes the candidate ineligible.

`evaluation.json` retains one private stable test ID, bucket, pass/fail/error status, and evidence reference per test before aggregation. ProgramBench MUST augment its official wrapper to retain this per-test result stream. If its pinned evaluator cannot do so, the candidate is ineligible rather than scored from aggregate Partial output.

Classify candidate-caused compile/import/dependency/assertion failures as non-passing behavior. Classify evaluator image, fixture, network, package, or harness failures as evaluator-invalid. Unknown errors are invalid, never candidate failures.

For runtime-valid rollout `r`, `S[r,i] = 1` only when node `i` reached `finished` and every test in `Q_i` passed; `model_failed` and `blocked` nodes are 0 even if another node incidentally implements their behavior:

```text
TaskSuccessRate(r) = sum(S[r,i] for i in V_g) / |V_g|
HiddenBehaviorPassRate(r) = passed tests in Q_g / |Q_g|
Resolved(r) = 1 iff every test in Q_g passes, else 0
```

For every arm/trial, macro-average each graph-level rate across the ten frozen graphs. Report benchmark-native pass/partial scores separately. If ProgramBench lacks a complete static node/test mapping, exclude the candidate before frame selection; never substitute Partial score for Task Success Rate.

## Telemetry and artifacts

**Proposed collector.** Run one collector per cell in every arm. It assigns a strictly increasing receipt-order `sequence`, records `monotonic_ns` relative to `arm_init` for intervals, and stores `wall_time_ms` only for display. Preserve `source_stream`, `source_event_id`, and `source_sequence`; never infer cross-stream order from coarse Stateful timestamps.

Every normalized record has `schema_version: "stateful.bench.event.v1"` and requires `run_id`, `trial_id`, `attempt_id`, `benchmark`, `instance_id`, `graph_id`, `arm`, `actor_kind: harness | agent | stateful`, `sequence`, `monotonic_ns`, `type`, and typed `payload`. Scheduler transitions require `task_id` but may omit `agent_id` before assignment. Arm-level harness records may omit both. After assignment, every agent/mutation/coordination record requires task and harness agent IDs; mapped `parallel-on` records additionally require `runtime_agent_id`.

**Proposed complete mutation observer.** Route every permitted mutation in every arm through the same observer/wrapper. Restrict mutation tools to instrumentable native edit/write and scoped sandbox write targets. Each mutation event contains `write_id`, path, operation, start/end monotonic times, `read_id`/`read_hash`, immediate `before_hash`, `after_hash`, outcome, and optional `denial_event_id`, `wait_id`, `retry_of`. Hash exact bytes; use `null` for an absent file.

For sandbox directory scope, snapshot before/after and emit one linked record per changed allowed-output file. Classify excluded cache/build/runtime paths separately. A changed allowed-output path without an originating tool/write ID invalidates coordination telemetry.

**Proposed artifact layout.**

```text
<run-root>/<benchmark>/<instance_id>/<graph_id>/<trial_id>/<attempt_id>/<arm>/
  task-graph.json
  run-manifest.json
  events.jsonl
  schedule-events.jsonl
  mutation-events.jsonl
  agents/<task_id>/{stdout.log,stderr.log,usage.json,node-terminal.json}
  stateful/{runtime-identity.json,agent-map.json,events.jsonl,identities/<task_id>.json}  # parallel-on only
  eval-commit.json
  evaluation.json
  instance-report.json
```

Write valid-attempt reports as `reports/{denovo,programbench}.{json,md}` and `reports/suite.{json,md}`. Reports reference evidence IDs rather than private tests or raw transcripts. Run roots stay private or gitignored; per-agent logs and runtime homes MUST NEVER be checked in. Scrub credentials/tokens/local absolute paths; exclude reasoning text and bearer values from normalized events and published reports.

**Proposed ordered Stateful export prerequisite.** Add append-only SQLite `INTEGER PRIMARY KEY` event `sequence` and:

```text
GET /v1/events?workspace_id=<id>&after_sequence=<n>&snapshot_max_sequence=<w>&limit=1000&order=asc
```

`after_sequence` is exclusive. The first request omits `w` and transactionally fixes/returns current maximum sequence as the snapshot watermark. Later pages return matching-workspace rows with `n < sequence <= w`, plus `next_sequence`, `snapshot_max_sequence`, and `has_more`. Gaps from other workspaces are legal; duplicates, regressions, rows above the watermark, and a changing watermark are not. After mapped agents finalize, drain that fixed snapshot before server stop. Later events require a new diagnostic snapshot and cannot enter confirmatory metrics.

Add append-only `ReservationGranted` at `promote_waiter_by_id`. Persist `server_monotonic_ns` from one process clock on `ReservationRequested` and `ReservationGranted`; grant records carry generic `wait_id`, normalized path/action, actual agent, and workspace fields. After drain, enrich task/attempt deterministically through frozen agent map; an unmapped grant is invalid. Queue wait is `grant.server_monotonic_ns - request.server_monotonic_ns` for one `wait_id`. Negative/missing links or server restart are invalid.

**Proposed normative source/formula table.**

| Source | Required fields | Derived metric/validity rule |
| --- | --- | --- |
| Scheduler transition | `task_id`, old/new state, cause | One legal state-machine transition per receipt sequence. |
| Model turn | `agent_id`, index, cap | Index is monotonic and never exceeds cap. |
| Tool | `tool_call_id`, tool, outcome | Tool volume; mutation tools must link to writes. |
| Read | `read_id`, path, hash | Supports stale-attempt detection. |
| Mutation | Full mutation fields above | Every allowed-output change is attributable. |
| Authorization | `decision_event_id`, reason, path/action, blocker, wait ID | Reason-coded denial/warning and collision linkage. |
| Reread/retry | `read_id`, `resolves_event_id`; `retry_of` | A conflict retry is denial → reread → successful retry. |
| Reservation | `wait_id`, path/action, request/grant/claim | Queue wait links one request to one grant. |
| Lifecycle | `runtime_agent_id`, workspace, type | Registration, heartbeat cadence, finalization completeness. |

After assignment every agent/coordination record resolves to task, attempt, harness agent, and—on `parallel-on`—runtime agent. Write overlap is duration with two agents' same-path attempt intervals intersecting. Agent overlap is duration with at least two `started..terminal` intervals active. Concurrency factor is summed active duration divided by union active duration. A cross-agent overwrite is a successful write whose immediate prior successful same-path writer was different and whose hash changes. A stale attempt has `read_hash != immediate before_hash`; a stale-write commit additionally succeeds. Blocked collision risk requires a blocker or stale hash; recovered collision requires the complete linked denial/reread/retry chain.

Use metric status `observed | not_applicable | unavailable | invalid`. Complete coverage with no initiating event is observed zero; incomplete started chains are `invalid`; inapplicable is `not_applicable`; intrinsically unobserved is `unavailable`. Never coerce absent evidence to zero.

## Metrics and decision rules

**Proposed reported metrics.** Correctness fields are Task Success Rate, Hidden Behavior Pass Rate, integration/regression pass, native score, and Resolved. Efficiency fields are observed whole-arm time, restricted time to correct, summed agent-active time, tokens, turns, tools, concurrency factor, and overlap time. Coordination fields are reason-coded denials/warnings, linked queue waits, all retries by reason, conflict-linked recoveries, same-path write overlap, cross-agent overwrites, stale-write commits, self-inflicted denials, and lifecycle completeness.

`self_inflicted_denials` is limited to `missing_reservation`, `missing_claim`, `stale_target_observation`, `missing_base_observation`, or `scope_mismatch` with no blocker. Report `blocked_collision_risk_count` and `recovered_collision_count`; never call either “prevented collision.” `duplicate_assignment_count == 0` is a validity invariant. Semantic duplicate investigation and semantic lost-edit count are `unavailable` in v1. Shared-checkout patch/merge failures are `not_applicable`. Report observable cross-agent overwrites, stale commits, final hashes, and behavioral correctness instead.

**Proposed terminal and retry rules.** Every wrapper terminal record has `origin: model | deadline | provider | container | harness`; unknown maps to `harness`. Model outcomes and whole-arm deadline censoring are scored and never independently retried. Provider/container/harness failures are runtime-invalid. Watchdog/cleanup failure is harness-invalid, not deadline censoring.

One logical `(benchmark, graph_id, trial_id)` attempt set includes all three arms in that trial's Latin-square order. If any member is runtime-invalid, exclude the entire attempt set from confirmatory inference and rerun all three arms from fresh cells under a new `attempt_id`, at most three attempt sets total. Retain every invalid artifact and origin. After three invalid sets, mark the confirmatory dataset `incomplete`; do not drop the graph or sample until success.

Runtime validity requires matching graph/base/reference/evaluator/image digests; complete node, hard-turn, finalizer, lifecycle, and fixed-watermark cursor ledgers; correct topology; clean provenance; complete evaluator output; `parallel-on` started-task/actual-agent bijection; and no unattributed mutation. Serialize `validity_status: valid | invalid | incomplete` and `validity_reasons: [{code, evidence_ids}]`; never infer validity from prose. Report invalid attempt counts/rates by arm and origin outside quality inference.

**Proposed paired inference.** Pair by `(benchmark, graph_id, trial_id)` and analyze benchmark families separately. For endpoint `Y`, compute `Y_bar[a,g] = mean_t(Y[a,g,t])`. With stratum sizes `(3, 4, 3)`, estimate:

```text
Delta = sum_s (n_s / 10) * mean_{g in G_s}(Y_bar[parallel-on,g] - Y_bar[parallel-off,g])
```

Per benchmark, build 10,000 stratified index sets with a fresh NumPy `Generator(PCG64(42))`: sample `n_s` graph IDs with replacement within each stratum, retain all trials/arms, and apply the same index sets to every endpoint/contrast. Recompute `Delta` for every set; use `numpy.quantile(replicates, [0.025, 0.975], method="linear")`. Use the analogous within-stratum estimator for stratum reports.

Quality non-inferiority requires both lower bounds:

```text
Delta_TaskSuccessRate >= -0.02
Delta_HiddenBehaviorPassRate >= -0.02
```

For rollout `r`, let `T_r = monotonic(EvalCommit) - monotonic(arm_init)`. `restricted_time_to_correct[r] = T_r` only when `Resolved(r) = 1` and `T_r <= H`; otherwise it is H. A commit after H is unresolved for this endpoint. Only after both quality gates pass, time superiority requires the upper CI bound of `Delta_restricted_time_to_correct` below 0.

Emit `verdict[denovo]` and `verdict[programbench]` independently: `incomplete` when that family lacks a complete confirmatory dataset; otherwise `stateful_superior` only when both quality gates and time superiority pass, `quality_inferior` only when either quality upper bound is below `-0.02`, `time_slower` only when both quality gates pass and time lower bound is above 0, else `inconclusive`. Never pool families. `suite_verdict` is `superior_in_both` only when both are superior, `incomplete` if either is incomplete, otherwise `mixed`.

Report `parallel-off - sequential` with the same stratified paired estimator/bootstrap, label it the **parallel-regime contrast**, and include all secondary/stratum metrics with CIs. Freeze margins, horizon, candidate frame, schedule, and analysis before results.

## Benchmark adapters and prerequisites

**Proposed DeNovo prerequisites.** Preserve outer `--max-concurrent 1`, prompt `v2`, isolation/proxy rules, and fresh evaluator. Repair compact propagation of `true_collisions_prevented`, `self_inflicted_denials`, and `scope_overlap_warnings`; describe `true_collisions_prevented` as a legacy field rather than causal proof. Add an end-to-end row/report test that exercises `write_orchestration_trace`, its compact returned object, result serialization, and nonzero aggregation.

**Proposed ProgramBench prerequisites.** Extend discovery/run metadata with upstream repository/commit, target-workspace/executable/image digests, immutable package/task-data/test-blob revisions, `smoke_compile_error`, and structured validity. Generated-code compile/test failure is a scored model outcome; Docker/copy/archive/evaluator infrastructure failure is container/harness-invalid. Graph mode bind-mounts each node container to one initialized airlock, removes per-agent copy-back, and retains one target container and finalized archive.

**Proposed shared runner/runtime prerequisites.** Add persistent sequential OMP control and terminal envelopes; hard model-turn gate/ledger; periodic graph heartbeat/finalization; cell collector; mutation observer; environment allowlist/path translation; serialized shared-repo joins; trusted `--allow-plain-http` preflight; cleared runtime overrides; task/actual-agent handshake; `ReservationGranted` plus request/grant `server_monotonic_ns`; and fixed-watermark cursor before efficacy.

**Proposed retained metadata.** Every run manifest retains Step-4 pins, cell/attempt identity, graph/evaluator/source/reference/image digests, arm order, endpoint without token, clock origin, collector/cursor completeness, hard-turn/terminal/lifecycle ledgers, structured validity, model-revision evidence, and terminal origins.

## Worked example

**Proposed four-node graph.**

```text
T01 -> {T02, T03} -> T04
```

`T01` establishes the public shared prerequisite. `T02` and `T03` are independent successors and may run together. `T04` is the public join and becomes ready only after both finish.

- The persistent sequential-session order is `T01,T02,T03,T04`.
- Parallel waves are `[T01]`, `[T02,T03]`, `[T04]`.
- With four nodes and the 500-turn aggregate cap, every node has a 125-turn cap.
- `parallel-off - sequential` is the **parallel-regime contrast**, not a concurrency-only claim.
- `parallel-on - parallel-off` is the sole Stateful contrast.
