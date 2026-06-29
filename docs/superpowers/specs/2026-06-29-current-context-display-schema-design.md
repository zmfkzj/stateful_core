# Current/Context Display Schema Cutover Design

## Goal

Reduce duplicate display data in `stateful current` and `state_context_render` by replacing the flat per-resource `items` display schema with grouped batch-oriented output.

## Current Behavior

`/v1/current` returns:

```json
{
  "status": "ok",
  "current": { "session_count": 0, "active_reservation_count": 1, "event_count": 1 },
  "items": [
    {
      "kind": "reservation",
      "severity": "info",
      "freshness": "live",
      "resource": "src/auth.ts",
      "purpose": "Fix auth validation behavior.",
      "session_id": "s1",
      "workspace_id": "w1"
    }
  ]
}
```

`/v1/context/render` returns the same flat `items` plus `prompt_text`. Multi-file reservations and same-purpose claims repeat `purpose`, `session_id`, and `workspace_id` once per resource. That no longer matches batch-level reservation/claim semantics.

## Chosen Approach

Use a server-response display cutover.

The store and authorization paths keep their existing internal `CurrentItem` model. The HTTP/MCP display boundary converts `Vec<CurrentItem>` into grouped display data before responding. This keeps policy behavior stable while changing the public display schema.

## New Response Schema

Both `/v1/current` and `/v1/context/render` return `groups` instead of `items`.

```json
{
  "status": "ok",
  "current": { "session_count": 0, "active_reservation_count": 1, "event_count": 1 },
  "groups": [
    {
      "kind": "reservation",
      "severity": "info",
      "freshness": "live",
      "session_id": "s1",
      "workspace_id": "w1",
      "purpose": "Fix auth validation behavior.",
      "resources": [
        {
          "path": "src/auth.ts",
          "summary": "Session s1 declared reservation for src/auth.ts.",
          "next_action": "Avoid overlapping edits to src/auth.ts unless coordinating with s1.",
          "evidence_kind": "declared_reservation",
          "source_refs": ["ReservationDeclared"]
        }
      ]
    }
  ]
}
```

`/v1/context/render` also returns `prompt_text` generated from `groups`.

## Grouping Rules

Group key:

```text
kind + session_id + workspace_id + purpose + freshness
```

Within each group:

- `severity` is the highest severity among grouped resources: `block > warn > info`.
- `resources` preserves the item order emitted by the store.
- Resource field names are display-oriented:
  - `path` replaces `resource`.
  - `summary`, `next_action`, `evidence`, `evidence_kind`, `source_refs`, `observed_at`, `expires_at`, and `age_seconds` stay resource-local.
- `purpose`, `session_id`, and `workspace_id` are group-level only.
- `items` is removed from both endpoints.

## Prompt Rendering

`render_prompt_text()` renders grouped output.

Example:

```text
Your Active Scope
- [info] reservation: Current session edits auth
  session: s1, workspace: w1
  resources:
  - src/auth.ts: This session declared reservation for src/auth.ts
  - src/session.ts: This session declared reservation for src/session.ts
```

Rules:

- Print `purpose` once per group.
- Print `session_id` and `workspace_id` once per group when present.
- Print each resource as a compact child bullet.
- `next:` remains resource-local and appears only for non-info severities, matching current behavior.
- Detailed mode keeps `evidence`; brief mode omits detailed evidence.

## Compatibility

This is an intentional public display schema cutoff. Tests and documentation should stop asserting `items` in `current` and `context_render` responses. Internal store tests may continue using `CurrentItem` where they exercise policy/state internals.

## Files Expected To Change

- `crates/stateful-core/src/context.rs`
  - Add display group/resource structs.
  - Add `ContextPackage::from_items()` grouping.
  - Render prompt text from groups.
- `crates/stateful-server/src/lib.rs`
  - Return `groups` for `/v1/current` and `/v1/context/render`.
- `crates/stateful-server/tests/routes.rs`
  - Update current/context tests to assert grouped schema.
- `docs/current-state-coordination.md`, `docs/implementation-contract.md`, `docs/state-model.md`, `docs/usage-reference.md`, and `README.md`
  - Update user-facing examples and schema descriptions if they mention flat `items` output.

## Testing

Use TDD:

1. Add or update a route test showing a multi-file reservation returns one `reservation` group with two resources and no top-level `items`.
2. Add or update a context render test showing prompt text prints purpose/workspace/session once per group.
3. Run the targeted server route tests.

## Non-Goals

- Do not change reservation/claim storage.
- Do not change authorization behavior.
- Do not add compatibility aliases for `items`.
- Do not add new dependencies.
