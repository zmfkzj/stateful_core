# Benchmark overhead reduction

## Goal

Reduce the observed `stateful:on` DeNovo/OMP overhead without weakening Stateful authorization. The change targets four measured pain points: verbose denial recovery, full stateful result payloads before summaries, repeated same-path denials, and empty-stop retry churn.

## Current behavior

Recent DeNovo runs showed `stateful:on` had lower score efficiency and much higher output/reasoning token use than `stateful:off` on the same paired instances. The largest output deltas came from hidden reasoning and retry churn, not useful final prose.

Relevant current seams:

- Denial text is built in `crates/stateful-cli/src/hook.rs::authorization_denial_reason` from server `AuthorizeDecision` responses.
- Stale write decisions are built in `crates/stateful-server/src/policy_service.rs::{stale_observation_decision, stale_claim_observation_decision}`.
- Context prompt text is rendered in `crates/stateful-core/src/context.rs::render_prompt_text` and returned by `crates/stateful-server/src/lib.rs::context_render` as `{current, items, prompt_text}`.
- Repeated same-path denial detection does not exist. Each denied write independently calls `/v1/authorize` and emits another `AuthorizationDenied` audit event.
- Empty stop detection does not exist in the DeNovo/Codex retry path. `crates/stateful-bench/scripts/codex_pair_agent.py::run_codex_with_resume` retries token/context-limit failures only; `crates/stateful-bench/scripts/denovo_codex_agent.py` maps successful empty stops to ordinary `stop` unless another failure path fires.
- OMP generated JS in `crates/stateful-cli/src/install.rs` formats empty sandbox/tool results as normal tool results and does not provide a quiesce signal.

## Requirements

- Keep Stateful authorization fail-closed.
- Do not turn denied writes into allowed writes.
- Do not steal claims or cancel other sessions.
- Keep normal maintainer workflow safe; benchmark-only paths may be narrower, but shared hook changes must improve or preserve existing behavior.
- Preserve existing `reason_code` values where callers may rely on them.
- Keep detailed context rendering available for manual inspection.
- Add tests before implementation for every behavior change.
- Prefer the shortest implementation that works; no new protocol layer unless the existing hook/server seams cannot carry the behavior.

## Design

Use a benchmark-first, shared-safe implementation. Keep policy semantics unchanged and reduce output at the formatting, retry, and summary boundaries.

### Concise denial recovery

Stale target and stale claim denials should give one mechanical next action.

Current stale guidance is longer and encourages agents to reason through procedure. Replace it with terse instructions:

```text
stale_target_observation -> Reread target, retry same edit with fresh base observation.
stale_claim_observation -> Reread target, reacquire claim, retry same edit.
```

`authorization_denial_reason` should continue to append wait details only when a `wait` record is present. It should not add ambient context or suggest `state_context_render` for routine denial recovery.

### Summary-first stateful results

Brief context output should start with a compact summary before item details.

Example shape:

```text
Stateful summary: 1 active claim, 2 reservations, 0 blocking, 3 nearby.
Your Active Scope
- src/foo.py: claimed for this session.
```

`RenderMode::Detailed` remains the verbose path. `RenderMode::Brief` should remain actionable but cheaper:

- always show the summary first when items exist;
- keep blocking and active-scope detail;
- omit repeated purpose lines for info-only nearby/stale items when the summary already names the counts;
- keep evidence text omitted in brief mode.

The server response can keep structured `items` for compatibility. The low-noise win is prompt text ordering and smaller brief output, not a breaking JSON shape change.

### Repeated same-path denial single-writer fallback

Detect repeated denials for the same write tuple:

```text
session_id + workspace_id + action + path + reason_code
```

On the second denial for the same tuple, keep denying but replace lengthy recovery prose with single-writer guidance:

```text
Repeated denial for src/foo.py. Stop retrying this path. Use one writer: parent/main agent owns the edit; subagents report findings only.
```

This should be a hook/server guidance improvement, not an authorization bypass. The fallback must not claim another session's resource or mutate the wait queue beyond existing behavior.

Ponytail implementation preference: hook-local dedupe state under the existing session marker/runtime area. Add a store/server table only if tests show hook-local state cannot observe the repeated denial cases we need.

### Empty-stop retry cap

Treat an empty successful stop as a distinct outcome instead of normal success.

Detection should require all of these:

- process exit code is success;
- no patch/output/tool progress is observed;
- assistant content is empty or whitespace;
- no meaningful result artifact was produced.

Behavior:

- One short retry is allowed only if the existing max-resume budget permits it.
- Retry prompt must be terse:

```text
Previous response was empty. Continue with the requested code change. Do not summarize.
```

- After the cap, finish as `empty-stop` or `codex-empty-stop`, not ordinary `stop`.
- Token/context-limit resume behavior remains unchanged.
- OMP empty tool output should render as a single line such as `No output.` rather than a repeated full object.

## Data flow

```text
Agent write -> hook authorize -> server policy -> Decision
                                -> hook denial formatter
                                -> concise or repeated-denial guidance

MCP/context render -> server current state -> ContextPackage
                                      -> summary-first brief prompt_text
                                      -> detailed prompt_text unchanged

Benchmark Codex run -> output parser -> empty-stop detector
                                 -> short retry or empty-stop finish_reason
```

## Testing plan

Write failing tests first.

- `stateful-server` route/policy tests:
  - stale target denial keeps `reason_code` and returns terse next action;
  - stale claim denial keeps `reason_code` and returns terse next action.
- `stateful-cli` hook tests:
  - first denial returns normal concise recovery;
  - second same tuple returns single-writer guidance;
  - different path/action/reason does not trigger repeated-denial guidance;
  - wait-denial guidance still includes `wait_id` and resume instructions.
- `stateful-core` context tests:
  - brief render starts with `Stateful summary`;
  - detailed render still includes evidence detail;
  - info-only nearby items are summarized without repeated purpose spam.
- `stateful-bench` Python adapter tests:
  - empty successful stop becomes `empty-stop` or one short retry;
  - token-limit resume behavior is unchanged;
  - non-empty success remains `stop`.
- OMP generated JS/hook tests where practical:
  - empty tool result renders as `No output.`

## Non-goals

- No new authorization policy semantics.
- No automatic claim stealing.
- No broad protocol redesign.
- No agent prompt strategy changes unrelated to the four requested overhead points.
- No new dependency.

## Rollout

Implement in this order:

1. concise stale denial messages;
2. summary-first brief context rendering;
3. hook-local repeated denial fallback;
4. empty-stop detection and compact empty output.

Run targeted tests after each change, then rerun the union of affected tests.
