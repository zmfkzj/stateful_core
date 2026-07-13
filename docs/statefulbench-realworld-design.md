# StatefulBench Real-World Corpus Design

**Status:** approved for implementation planning  
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

Every arm/trial starts from a fresh checkout created from the verified source archive.

- `sequential`: ten independent task agents run in specification order.
- `parallel-off`: ten task agents run concurrently in one checkout without Stateful coordination.
- `parallel-on`: ten task agents run concurrently in one checkout and share one arm-local enforcement server.

The final agent runs only after all task-agent processes have been reaped. No checkout or Stateful server is shared across repositories, arms, or trials.

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

Each detailed result row also contains `coordination_metrics`. It supplements the efficiency fields and does not redefine `cleared`.

- `sequential` and `parallel-off` rows set `coordination_metrics` to `null`.
- A `parallel-on` row contains an object only when all coordination diagnostics are complete. If the private coordination SQLite snapshot is unavailable, locked, malformed, or lacks the expected schema; server-log markers cannot be captured consistently; marker phase counts decrease; or the result assembler cannot construct the object, the field is `null` and the existing diagnostic failure path leaves the row uncleared.
- Each populated object contains:
  - `notifications.by_kind`, a status-count map for protocol notification kinds. It always includes `scope_overlap` and `reservation_granted`, each with `created`, `delivered`, `pending`, and `expired` counts; other protocol kinds may appear as sorted keys. `created` is the sum of retained status counts. A `scope_overlap` count is a deduplicated advisory-notification count, not a raw edit-collision count, and `delivered` records poll/SSE delivery.
  - `waits.by_final_status`; `waits.grant_wait_time_s` with `count`, `total`, `mean`, and `max`; and `waits.unmeasured_grants`. A measurable grant duration runs from the matched reservation wait request to grant availability. Missing links or timestamps, malformed timestamps, and negative durations increment `unmeasured_grants` rather than becoming zero; `mean` and `max` are `null` when `count` is zero.
  - `authorization.denied_by_reason` and `authorization.warned_by_reason`, which count protocol reason codes without decision messages.
  - `context_renders.server` and `context_renders.explicit_tool_calls`, each with `tasks`, `final`, and `total`. Server `tasks` is the `after-tasks - before-tasks` successful-marker delta, `final` is `after-final - after-tasks`, and `total` is their sum. Explicit `tasks` sums task-agent OMP-log executions and `final` comes from the final-agent log. The benchmark does not derive an automatic-render count.

`results.json` and `summary.json.results` retain complete row objects. A repository/arm aggregate has coordination metrics only when every scheduled `parallel-on` row is present and complete; otherwise its field is `null`, never a partial total. Aggregate notification, wait-status, authorization, render, measured-grant, and unmeasured-grant counts and wait totals are summed. The aggregate wait `mean` is the weighted `total / count`, rather than an average of row means; `max` is the maximum row value, or `null` for zero measured grants.

The published object is value-free: it contains aggregate protocol categories, counts, and derived aggregate wait durations only. It must not expose notification payloads, wait/reservation/agent IDs, paths, resources, timestamps, free-form messages, raw database rows, or raw server-log lines. These diagnostics are observational evidence, not proof that Stateful caused an outcome. A fresh authorized smoke must emit the new fields before they are used for an on/off conclusion.

The reporting contract remains efficiency-only. A single trial is descriptive. Results must not be presented as behavioral-quality, causal, safety, or statistical-superiority evidence.

## Qualification and launch gates

Run the following gates in order:

1. Verify every pinned source, setup procedure, and upstream suite.
2. Demonstrate task-level RED on base and GREEN on the reference patch.
3. Demonstrate repository-level GREEN for the integrated ten-task reference patch.
4. Validate zero isolated nodes in every overlap graph.
5. Run the generated deterministic baseline smoke.
6. Run one repository through all three live arms.
7. Run all ten repositories through the full three-arm benchmark.
8. Preserve and report result artifacts and the provenance manifest.
