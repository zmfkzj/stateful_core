# DeNovoSWE Stateful Coordination Anomaly Analysis (2026-07-04 Run)

Last updated: 2026-07-04

Root-cause analysis of the unexpectedly high Stateful coordination activity
observed in the `stateful:on,subagent:on` condition of DeNovoSWE run series
`s20260704-gpt55-10x3-omp-docker-subagent-on`, with concrete fixes.

## Run Under Analysis

| Field | Value |
|---|---|
| Run series | `s20260704-gpt55-10x3-omp-docker-subagent-on` |
| Trials | `denovo-t1`, `denovo-t2`, `denovo-t3` (10 instances each) |
| Condition analyzed | `stateful:on,subagent:on` |
| Model / reasoning | `openai-codex/gpt-5.5` / `high` |
| Agent | `omp-cli`, Docker image `stateful-omp-agent-gpt55-20260704:latest` |
| Concurrency | `--max-concurrent 4`, one shared Stateful server per host |
| Artifacts root | `/Users/arthur/stateful_bench_runs/denovo/runs/<run-id>/` |

Source artifacts: `denovo-report.generated.json` per trial, per-instance
`orchestration-trace.json`, per-instance OMP session JSONL under
`omp-homes/*/home/.omp/profiles/stateful/agent/sessions/`.

All numbers below were independently re-derived from the artifacts and
confirmed with zero discrepancies.

## Executive Summary

Three independent defects present in the analyzed run, not agent-model behavior,
explain most of the anomalous coordination activity:

1. **The permitted bash path was unusable inside the benchmark container.**
   Every `stateful sandbox run` profile on Linux execs `bwrap`, and the
   benchmark launched the agent container without the privileges bwrap needed.
   Result: 218/218 bash tool calls failed (0 successes) across all stateful-on
   rollouts.
2. **The write-boundary recovery loop could not complete.** On a denial
   (notably `stale_target_observation`) the pre-tool hook released the
   auto-acquired claim, but no retry path could reacquire a claim for an active
   auto-declared reservation, and the OMP extension exposed no manual
   reservation/claim tools. Result: retry storms — 580 failed
   edit/write/lazy-resume tool results across 30 rollouts — until the
   repeated-denial guard fired.
3. **Per-instance orchestration traces were not instance-local.** Trace capture
   stored the server's unfiltered latest-100 global events, so 27 of 30 trace
   files contained events from up to 4 concurrent instances' workspaces. Raw
   trace sums overstated per-instance activity (362 deduped denials vs 107
   official), while official summary counts were floor values truncated by the
   100-event window.

A contributing factor in the analyzed run: the benchmark agent profile advertised
unrestricted tool access ("FULL access to all tools (edit, write, bash, ...)"),
while policy plus the broken bwrap path made bash effectively unavailable, so
agents burned turns probing (`--help`, `--version`, `--no-bwrap`, raw bash,
heredocs) instead of solving the task.

## Implementation Status

The current implementation has landed RC1-A, RC2-A/B, and RC3-B plus the trace
capture update: Docker OMP runs add `--cap-add SYS_ADMIN --security-opt
seccomp=unconfined --security-opt apparmor=unconfined --security-opt systempaths=unconfined`; OMP native `edit`/`write`
can reacquire missing same-reservation claims and keep auto-claims on `stale_target_observation`; and
`/v1/events` accepts `workspace_id`, `since`, and `limit` while trace capture
requests `workspace_id` plus `limit=1000` and records
`events_window_saturated`. These fixes remove the known orchestration defects;
the full rerun in the validation plan is still required before making
stateful-on/off quality claims.

## Phase 4 Gate Decision

The cross-agent findings-sharing lever remains gated off. Current artifacts do
not show measured re-derivation across subagents, such as repeated identical
exploration or repeated same-file failures across sibling subagents. The
available contention signal is limited to four true `active_claim_conflict`
events across the 30 analyzed stateful-on rollouts, and the full rerun after the
known orchestration fixes is still required before making stateful-on/off
quality or coordination-value claims.

Decision: do not implement `resource_finding` storage, server, extension, or
context rendering until a rerun or contention slice shows re-derivation evidence
that this feature would remove.

## Symptoms and Verified Measurements

### S1. Denials without peer contention

Official report totals (`orchestration_denial_messages`, stateful-on, 3
trials; per-trial `orchestration_denial_events`: t1=32, t2=33, t3=42):

| Denial message | Count |
|---|---:|
| `Supported writes require active file or directory reservation.` (`missing_reservation`) | 46 |
| `Target existence changed since the supplied base observation.` (`stale_target_observation`) | 40 |
| `Hook file targets require exact active same-reservation file claims for every affected path.` (`missing_claim`) | 20 |
| `Target is outside active reservation scope.` (`scope_mismatch`) | 1 |
| Total | 107 |

True cross-agent contention was rare: deduplicating raw trace events by
`event_id` across all 30 stateful-on rollouts yields only 4
`active_claim_conflict` events, and every one carries a non-null
`wait.blocking_agent_id`. There was no case of a claim conflict without an
actual blocking claim. The `missing_reservation` volume is the write-first
flow (agents write, the boundary auto-declares afterward), not contention.

### S2. Edit/write failures while holding a claim

Failed tool results in stateful-on OMP sessions (main + subagent JSONL):

| Tool result | Count |
|---|---:|
| `edit` failed, total | 256 |
| — of which `Reread target, retry same edit with fresh base observation.` | 236 |
| `write` failed, total | 107 |
| — of which same reread/retry message | 91 |
| `lazy_edit_resume` failed, total | 147 |
| — `Acquire matching same-reservation file claims ...` | 123 |
| — HTTP 409 `reservation not found` | 15 |
| `lazy_write_resume` failed, total | 70 |
| — `Acquire matching same-reservation file claims ...` | 55 |
| — HTTP 409 `reservation not found` | 11 |
| Repeated-denial guard (`Use one writer: parent/main agent owns the edit`) | 10 |

Subagents escalated this to their parent agents over IRC — e.g.
"active tools expose no state_reservation/state_claim tools here"
(CompilerCLI) and "no claim tools are exposed here" (RuntimeComposite) —
recorded as `irc:incoming` custom messages in the main-session JSONL
(e.g. t1 `aurzenligl_prophy_pr33`, session
`2026-07-04T12-40-08-660Z_019f2d24-....jsonl`). They could not repair the
denial themselves.

### S3. Bash tool failures

All bash tool calls in stateful-on sessions failed: 218 calls, 218 errors,
0 successes.

| First line of failure | Count |
|---|---:|
| `bwrap: No permissions to create new namespace, ...` | 86 |
| `Bash wrapper must be a single stateful sandbox command` | 53 |
| `OMP raw Bash is denied; use the trusted stateful sandbox command` | 26 |
| Unsupported `stateful sandbox run` args (`--help`, `--version`, `--no-bwrap`, `--`, inline `python -c`, ...) | ~40 |
| Other (reservation-declare guidance, malformed shapes) | remainder |

### S4. Abnormal trace shape

- 27 of 30 per-instance `orchestration-trace.json` files contain events
  from more than one `workspace_id` (max 4 distinct — matching
  `--max-concurrent 4`).
- Raw AuthorizationDenied events deduped by `event_id` across all traces:
  `missing_reservation` 204, `stale_target_observation` 93,
  `missing_claim` 53, `scope_mismatch` 8, `active_claim_conflict` 4 —
  total 362, vs 107 in the generated reports.
- Trace headers embed global server state, e.g.
  `context.current.agent_count: 653`, `event_count: 1282`.
- Many `ActivityFinalized` events carry `completed_reservations: 0,
  released_claims: 0` (finalize of agents that held nothing), inflating
  perceived lifecycle traffic.

## Root Causes

### RC1: bwrap-based sandbox was unusable inside the benchmark container

Mechanism in the analyzed run (references are to the analyzed revision):

- Every Linux sandbox profile execs `bwrap`: shell profiles via
  `bubblewrap_command` (`crates/stateful-cli/src/sandbox.rs:3158-3178`,
  dispatch at `:784-798`), git via `bubblewrap_git_command`
  (`:3181-3194`, dispatch `:857-871`), github-pr via
  `bubblewrap_github_pr_command` (`:3197-3213`, dispatch `:913-919`).
- bwrap args always unshare namespaces: `--unshare-all`,
  `--die-with-parent` (`crates/stateful-cli/src/sandbox.rs:3269-3277`).
- There is no user-facing no-bwrap flag or capability-detection fallback.
  The bash-wrapper parser rejects anything unknown
  (`crates/stateful-cli/src/sandbox/parse.rs:45-168`), which produced the
  observed `unsupported stateful sandbox run argument` failures. The only
  bypass is the internal nested-sandbox gate
  `STATEFUL_SANDBOX_RUN_ACTIVE` + `STATEFUL_ALLOW_NESTED_SANDBOX_RUN=1`
  (`crates/stateful-cli/src/sandbox.rs:929-936`).
- The benchmark's agent container was started without namespace privileges in
  the analyzed run: `docker run --rm --network bridge --workdir /workspace`
  plus mounts and env only (`crates/stateful-bench/scripts/denovo_codex_agent.py`
  at the time; confirmed in recorded `codex-command.json` — no `--privileged`,
  `--cap-add`, `--security-opt`, or userns options).
- The image does install bubblewrap and verifies it at build time
  (`crates/stateful-bench/docker/denovo-omp-agent.Dockerfile:9-32`), so
  this is a runtime-privilege failure, not a missing binary.
- `--agent-docker-sandbox off` does NOT affect this: it only sets
  `STATEFUL_OMP_SANDBOX=off` in the container env
  (`crates/stateful-bench/scripts/denovo_codex_agent.py:907-923`; flag
  definition `crates/stateful-bench/src/denovo.rs:53-68,145-150`). It
  gates OMP's own sandbox, not stateful-cli's bwrap.

Consequence: the only policy-permitted bash path failed 100% of the time,
agents could not run any verification (pytest, python -c, git status), and a
large share of turns was spent probing alternative bash shapes.

### RC2: write-boundary denial recovery could not complete

Intended lifecycle (declare -> claim -> transact -> release):

- Pre-tool authorize per target: `crates/stateful-cli/src/hook.rs:541-555`.
- On first `missing_reservation`/`scope_mismatch` for OMP edit/write, the
  boundary auto-declares an exact-file reservation and acquires
  same-reservation claims (`crates/stateful-cli/src/hook.rs:565-585,
  831-898`), then reauthorizes.
- On success, the claim is released in the post-tool hook after the
  transaction (`crates/stateful-cli/src/hook.rs:1043-1058,1091-1112`).

Where it broke in the analyzed run:

1. When reauthorization blocked — the dominant case was
   `stale_target_observation` from server freshness validation
   (`crates/stateful-server/src/policy_service.rs:919-995,1354-1359`) —
   the pre-hook **released the auto-claim before returning the block**
   (`crates/stateful-cli/src/hook.rs:588-603`).
2. The agent reread and retried as instructed, but then had an active
   reservation and **no claim**. Exact hook-file writes require an active
   same-reservation claim
   (`crates/stateful-server/src/policy_service.rs:731-759,440-459`), so
   the retry failed with `missing_claim`.
3. Nothing reacquired the claim: the server's claim-on-authorize path only
   consulted wait-queue reservations (`status = 'reserved'`)
   (`crates/stateful-server/src/policy_service.rs:1058-1084`,
   `crates/stateful-store/src/reservations.rs:620-655`), never the active
   reservation created by auto-declare
   (`crates/stateful-store/src/lib.rs:715-718`).
4. `lazy_edit_resume`/`lazy_write_resume` claimed only when the queued
   operation had a `wait_id`
   (`crates/stateful-cli/assets/stateful-omp-extension.js:1148-1161`);
   auto-declared active reservations had none, so resume reran authorization
   and failed the same way. When a wait_id existed but the wait-queue entry
   expired (`CLAIMABLE_RESERVATION_TTL_SECONDS = 120`,
   `crates/stateful-store/src/lib.rs:28-31`), was claimed, or was finalized,
   `stateful reservation claim` returned HTTP 409 `reservation not found`
   (`crates/stateful-server/src/policy_service.rs:1114-1122`,
   `crates/stateful-store/src/reservations.rs:793-797`) — the 26 observed
   409s.
5. The extension registered only `lazy_edit_resume`, `lazy_write_resume`,
   `lazy_bash_resume`
   (`crates/stateful-cli/assets/stateful-omp-extension.js:1170-1318`); this
   OMP profile exposed no `state_reservation_*`/`state_claim_*` tools for
   manual repair, and hook guidance assumed that absence, directing agents to
   auto-declare/lazy resume instead
   (`crates/stateful-cli/src/hook.rs:1870-1872`).
6. Cross-agent retries were worse: active reservations are keyed by
   `agent_id` + `workspace_id` (`crates/stateful-store/src/lib.rs:715-718`)
   and each OMP subagent gets a distinct derived agent id
   (`crates/stateful-cli/assets/stateful-omp-extension.js:123-128`), so a
   different subagent retrying the same path saw `missing_reservation`.
7. After repeated failures the hook's repeated-denial guard fired
   (`crates/stateful-cli/src/hook.rs:1450-1465`): "Use one writer:
   parent/main agent owns the edit; subagents report findings only."

Consequence: the reread-and-retry instruction embedded in the denial was not
actually satisfiable in that environment; agents looped through
edit -> stale -> reread -> missing_claim -> lazy resume -> denial until the
guard stopped them. This inflated `missing_reservation`/`missing_claim` denials
and uncached token usage.

### RC3: orchestration traces were captured from global server state

Mechanism in the analyzed run:

- Trace capture called `GET /v1/current` and `GET /v1/events` with no
  parameters (`crates/stateful-bench/scripts/denovo_codex_agent.py` at the time).
- The server's `/v1/events` handler ignored query state and returned
  `store.recent_events(100)` — the latest 100 events across ALL workspaces
  (`crates/stateful-server/src/lib.rs`, `crates/stateful-store/src/lib.rs` at
  the time).
- The raw list was written to the trace unfiltered. Only the summary fields
  filtered by the instance's `STATEFUL_WORKSPACE_ID`
  (`summarize_orchestration_events`, `denovo_codex_agent.py` at the time).
- The per-condition report aggregates those per-instance workspace-
  filtered summaries (`orchestration_trace_summary`,
  `crates/stateful-bench/src/denovo.rs:2043-2208`).

Consequences:

- Raw `events` arrays were global latest-100 snapshots: with 4 concurrent
  instances sharing one server, 27/30 traces contained foreign-workspace events.
  Summing raw traces overstates per-instance activity (362 vs 107).
- Official summary counts were truncated by the same 100-event window: an
  instance whose workspace-filtered match count hit 100 (observed) had saturated
  the window, so official denial/heartbeat/event counts are **floor values**, not
  exact totals, for this analyzed run.

## Solutions

### RC1 fix options (RC1-A landed)

| Option | Change | Tradeoff |
|---|---|---|
| A. Grant bwrap-capable privileges to the agent container (recommended) | Add `--cap-add SYS_ADMIN --security-opt seccomp=unconfined` (validate; fall back to `--privileged` if insufficient on the runner) in the Docker command builder `crates/stateful-bench/scripts/denovo_codex_agent.py:924-956`; optionally plumb a flag beside `DeNovoAgentDockerSandbox` (`crates/stateful-bench/src/denovo.rs:53-68,622-631`) | Keeps `stateful sandbox run` semantics identical to production; broadens container privileges (benchmark-only risk) |
| B. Explicit container-direct mode in stateful-cli | Add a named opt-in (env/flag) that skips bwrap at the three dispatch sites `crates/stateful-cli/src/sandbox.rs:761-765,833-838,897-902`; benchmark sets it in `denovo_codex_agent.py:921-956` | Works without Docker privileges but removes Linux FS/process isolation for sandboxed commands; must be loud and opt-in |
| C. Temporary: reuse nested-sandbox env gate | Set `STATEFUL_SANDBOX_RUN_ACTIVE=1` + `STATEFUL_ALLOW_NESTED_SANDBOX_RUN=1` in container env (`crates/stateful-cli/src/sandbox.rs:929-936`) | Zero code change, but semantically lies ("already inside a sandbox"); unblocker only |

### RC2 fix options (RC2-A/B landed)

| Option | Change | Tradeoff |
|---|---|---|
| A. Auto-reacquire claim on `missing_claim` (recommended) | In `authorize_omp_targets` denial handling (`crates/stateful-cli/src/hook.rs:565-603`), on OMP edit/write `missing_claim` call a helper (beside `declare_and_claim_omp_pre_tool_reservation`, `:831-912`) that POSTs `/v1/claim/acquire` for the target paths under the existing active reservation, then re-loop | Smallest fix; fails closed when no active reservation exists; does not (and should not) let a different agent steal another agent's reservation |
| B. Keep the auto-claim across stale denials | Do not `release_omp_auto_claims` when the block reason is `stale_target_observation`/`missing_base_observation` (`crates/stateful-cli/src/hook.rs:588-603`); post-tool release after the successful retry stays as is | Directly fixes reread-retry for the same agent; holds the claim during the reread (claim TTL 300s) so peers wait slightly longer |
| C. Expose claim tools in the OMP extension | Register `state_claim_acquire`/`state_reservation_claim` tools near the lazy tools (`crates/stateful-cli/assets/stateful-omp-extension.js:1148-1318`); align guidance `crates/stateful-cli/src/hook.rs:1870-1872` | Gives subagents manual recovery; increases tool surface and misuse risk |

Additionally worth considering server-side: teach claim-on-authorize to
also match active reservations owned by the same agent
(`crates/stateful-server/src/policy_service.rs:1058-1084`), which makes
the denial text ("write boundary can claim") truthful for auto-declared
reservations.

### RC3 fix options (RC3-B plus trace capture update landed)

| Option | Change | Tradeoff |
|---|---|---|
| A. Filter at capture | In `write_orchestration_trace` (`denovo_codex_agent.py:2248-2259`), filter `events` by `STATEFUL_WORKSPACE_ID` before writing, and record `events_window_saturated = (len(raw) == 1000)` | Instance-local traces immediately; still window-capped |
| B. Server-side query params | Add `workspace_id`/`since`/`limit` params to `/v1/events` (`crates/stateful-server/src/lib.rs:203-208`) backed by a filtered store query beside `recent_events` (`crates/stateful-store/src/lib.rs:1017-1033`); benchmark passes its workspace id and limit | Exact per-workspace counts, removes the floor-value caveat when the window is not saturated; small server+store+client change |
| C. Analysis hygiene rule | Document: never sum raw `orchestration-trace.json` events; always dedupe by `event_id` and filter by the instance `workspace_id`; prefer `denovo-report.generated.json` | No code; prevents mis-reporting for historical runs and any saturated event window |

### Contributing-factor fix

Align the benchmark agent profile text (`quick_task.md`/`task.md` "FULL
access to all tools" in the OMP home template) with reality, or rely on
RC1 fixes making bash genuinely usable. Keeping prompts truthful avoids
probing loops; per benchmark policy, do not add task-strategy hints.

## Validation Plan

1. Rebuild the agent image from the current repo, apply the chosen RC1 +
   RC2 fixes, then run a single-instance smoke rollout
   (`stateful:on,subagent:on`). Gate on:
   - at least one successful `bash` tool result (`stateful sandbox run
     --fs read-only ... 'git status'` succeeds inside the container);
   - zero `missing_claim` denials following a `stale_target_observation`
     retry for the same agent and path;
   - repeated-denial guard count = 0.
2. Verify trace shape: every new `orchestration-trace.json` has exactly
   one distinct `workspace_id` among events (after RC3-A/B) and records
   window saturation.
3. Re-run the full 10x3 comparison with fresh run IDs (never reuse the
   `s20260704` IDs) per `docs/denovo-benchmark-guide.md`, and only then
   compare `stateful:on` vs `stateful:off` quality.

## Impact on Interpreting the 2026-07-04 Results

- The `stateful:on` quality deficit in this series (mean score 0.466 vs
  0.548 off; success 4/30 vs 6/30) is confounded by RC1/RC2: rollouts
  spent turns on a bash path that can never succeed and on write-retry
  loops. Do not attribute the gap to coordination overhead semantics
  until a rerun with fixes.
- Higher `stateful:on` token usage (uncached 120,847 vs 32,328) is partly
  retry-loop inflation from RC2 and probing from RC1.
- Lifecycle validity (AgentRegistered / repeated AgentHeartbeat /
  ActivityFinalized present) is unaffected and remains valid evidence.
- Coordination-activity metrics: cite only `denovo-report.generated.json`
  values, labeled as floor values; never cite raw trace sums (RC3).

## Appendix: Verification Notes

Every number in this document was re-derived from artifacts on
2026-07-04 via workspace-filtered jq queries (official reports), event-id
deduped raw-trace scans, and OMP session JSONL tool-result scans (main
sessions plus subagent subdirectories); expected vs measured matched with
zero discrepancies, including: official denials 32/33/42 (total 107) and
message breakdown 46/40/20/1; raw deduped denials 362
(204/93/53/8/4); bash results 218 failed / 0 succeeded (86 bwrap, 53
wrapper-shape, 26 raw-bash-denied); failed tool results edit 256 / write
107 / lazy_edit_resume 147 / lazy_write_resume 70; HTTP 409
`reservation not found` 26; repeated-denial guard 10; claim conflicts 4,
all with `blocking_agent_id`; 27/30 traces with >1 workspace_id (max 4).
