# Plan-only context rendering

## Goal

Reduce `context_render` from a denial-time recovery mechanism to a planning-time coordination aid. Agents should see other active sessions early enough to choose a safe plan, but hook denials should no longer call `/v1/context/render` repeatedly just to explain state that edit resume and targeted denial payloads already handle.

## Current behavior

`/v1/context/render` is used by several independent paths:

- `UserPromptSubmit` renders prompt-ready context once per session marker.
- The command-policy reminder tells agents to inspect current state with `state_current_read` or `state_context_render`.
- `stateful-command-policy` lists context inspection as the first default write-flow step.
- Codex `PreToolUse` wraps Bash, edit, write, and apply_patch decisions with live context rendering.
- Sandbox authorization denial bodies attach `current_state` by calling `/v1/context/render`.
- The MCP `stateful/context` resource maps to `state_context_render`.

These paths do not share a per-turn or per-resource cache. A single agent turn can therefore render context at prompt submit, again because the injected guidance tells the model to inspect context, again during tool authorization, and again in denial enrichment.

That made sense when agents needed to observe other sessions' claims manually. Edit resume now moves that recovery path into Stateful coordination primitives: missing claims, queued reservations, claimable waits, and stale observations have direct next actions. Denial-time ambient context is now mostly noise.

## Requirements

- Keep `/v1/context/render` and the `state_context_render` MCP tool available for explicit planning and manual inspection.
- Stop automatic denial-time calls to `/v1/context/render`.
- Stop automatic `PreToolUse` calls to `/v1/context/render` for Bash, edit, write, and apply_patch authorization.
- Stop sandbox authorization denial bodies from calling `/v1/context/render` to attach `current_state`.
- Keep direct denial payloads actionable without ambient context: missing claim, claim conflict, stale observation, and queued/reserved wait records should name the next Stateful action.
- Keep prompt-time context rendering at most once per session marker where the hook already supports it.
- Update agent-facing guidance so `context_render` is described as planning/manual inspection, not as a required default write-flow step.
- Avoid adding a new cache unless later evidence shows plan-time rendering still duplicates too much.

## Design

Treat `context_render` as one feature with one purpose:

```text
Planning-time coordination context
```

It answers: "Before I choose a plan, is another active session working nearby or blocking a resource I should account for?"

It no longer answers: "A write was denied; what ambient context should I append to the denial?"

### Automatic rendering

Allowed automatic path:

```text
UserPromptSubmit -> /v1/context/render -> prompt text
```

This remains guarded by the existing marker:

```text
.stateful_core/runtime/prompt_context/{session_id}.sent
```

No new automatic render should be introduced for tool authorization or sandbox denials.

### Manual rendering

Keep these interfaces:

```text
state_context_render(mode, resource?)
stateful/context MCP resource
POST /v1/context/render
```

Their documentation should say they are for planning or manual inspection. If an agent already inspected context for the current turn and same resource, it should reuse that information unless a hook denial reports stale/missing state or the target resource changed.

### Denial handling

Deny responses should stay narrow and mechanical.

Examples:

```text
missing_claim -> acquire an exact same-session claim for the denied path
claim_conflict -> wait for/resume the queued reservation or claim a ready wait id
stale_target_observation -> re-read the target and retry with a fresh observation
missing_reservation -> declare/request the exact file reservation
```

Do not append rendered nearby activity, stale sections, or other active-session context to these denials. If the denial needs structured data, include only data needed for the next action, not a rendered planning summary.

### PreToolUse hooks

Remove or bypass the `with_file_tool_live_context` behavior for automatic rendering. The authorization decision should return allow/deny based on target authorization only.

For allow decisions, do not call `context_render` just to see whether an actionable context message exists. That speculative check is the main repeat-call source.

For deny decisions, return the denial reason from authorization. Do not render ambient current state.

### Sandbox authorization

Remove context rendering from sandbox authorization denial bodies. The body should include:

```text
status
message
allowed_write_targets
denied_write_targets
```

It should not include `current_state` fetched through `/v1/context/render`.

## Agent guidance changes

Move context inspection out of the default write flow.

New guidance shape:

```text
Planning: inspect context once when choosing a plan or when the target resource is unknown and active coordination may matter.
Writing: declare the exact file set, acquire exact same-session claims, then write.
Denial recovery: follow the denial's next action; do not call context_render unless you need to revise the plan.
```

The `stateful-command-policy` skill should say:

- `state_context_render` is optional planning/manual inspection.
- `state_current_read` is broad state inspection when target paths are unknown.
- Neither tool is a mandatory first step for every write or every denial.
- If context was already inspected this turn for the same resource, do not repeat it.

The hook reminder should stop saying "First inspect current state" as a blanket precondition. It should instead point to reservation and claim flow, with planning context as optional.

## Non-goals

- Do not remove the server endpoint.
- Do not remove the MCP tool.
- Do not redesign edit resume.
- Do not add a context-render cache in the first pass.
- Do not make deny messages verbose again through a different endpoint.

## Testing

Update tests that currently assert automatic context rendering.

Expected removals or inversions:

- Bash pre-tool denial should not post `/v1/context/render`.
- edit/write/apply_patch authorization should not post `/v1/context/render`.
- apply_patch denial should not append rendered context.
- sandbox authorization denial should not include `current_state` from `/v1/context/render`.

Expected retained coverage:

- `UserPromptSubmit` still posts `/v1/context/render` once and writes the session marker.
- A second `UserPromptSubmit` for the same session still skips rendering.
- Manual `state_context_render` MCP calls still map to `POST /v1/context/render` and enrich session/workspace/repo identity.
- The `stateful/context` resource still maps to `state_context_render` if the resource remains exposed.
- Denial payload tests assert direct next-action content rather than ambient context text.

## Documentation updates

Update these docs and generated guidance:

- `stateful-command-policy/SKILL.md`
- `omp-stateful-required-rule.md`
- README or usage references that describe default Stateful write flow
- architecture/current-state docs that describe `context_render` as prompt/denial context
- hook tests and generated install-time docs that embed the old reminder text

The docs should consistently call `context_render` a planning/manual inspection tool.

## Risks

- Agents may lose helpful context on denials. This is acceptable because denial recovery should be mechanical after edit resume.
- Existing tests encode the old behavior heavily. The implementation should update those tests rather than preserve the automatic calls.
- If planning context is still useful in OMP, OMP needs guidance rather than a user-prompt hook path, because OMP user-prompt-submit is currently unsupported.

## Success criteria

- A normal write denial does not call `/v1/context/render`.
- A normal Bash/tool authorization path does not call `/v1/context/render`.
- Sandbox write-target denial does not call `/v1/context/render`.
- Prompt/manual planning context still works.
- Agent-facing guidance no longer makes `context_render` a mandatory first step for every write or denial.
