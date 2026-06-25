# OMP LSP Allowlist and Stateful MCP Resources Design

## Goal

Fix two OMP integration gaps:

1. `lsp` should be allowed by default for OMP-managed repos.
2. `mcp://stateful/current` should resolve through the Stateful MCP server instead of failing with no resources.

The selected approach exposes the small useful Stateful read resources, not only `current`.

## Scope

Implement read-only MCP resources for:

- `stateful/current` -> `GET /v1/current`
- `stateful/events` -> `GET /v1/events`
- `stateful/context` -> `POST /v1/context/render` with default arguments

Add `lsp` to OMP default repo tool allowlist.

Out of scope:

- Write-capable MCP resources.
- Resource templates with arbitrary path/query arguments.
- New OMP config settings.

## Existing Code

- `crates/stateful-cli/src/repo_registry.rs` defines `DEFAULT_OMP_ALLOWED_TOOLS`.
- `crates/stateful-cli/src/mcp.rs` handles JSON-RPC methods for tools only.
- `crates/stateful-cli/tests/cli.rs` checks the generated default allowed tools.
- `crates/stateful-cli/tests/mcp.rs` checks MCP JSON-RPC behavior.

## Behavior

### OMP allowlist

Newly enabled repos include `lsp` in `allowed_tools` beside existing OMP defaults.
Existing custom allowlists are preserved by the current merge path.

### MCP resources

`initialize` advertises resources capability.

`resources/list` returns fixed resource descriptors:

- URI: `stateful/current`, name: `current`, MIME type: `application/json`
- URI: `stateful/events`, name: `events`, MIME type: `application/json`
- URI: `stateful/context`, name: `context`, MIME type: `text/plain`

`resources/read` accepts `params.uri`.

- `stateful/current` returns the same JSON body as `state_current_read`.
- `stateful/events` returns the same JSON body as `state_events_read`.
- `stateful/context` calls `state.context.render` with `{ "mode": "brief" }` and returns the server text/body; omitting `resource` avoids filtering context to an empty path.
- Unknown URIs return JSON-RPC `-32602`.

Errors from the backing HTTP call are returned as MCP resource content only when the HTTP request succeeds at transport level; JSON-RPC infrastructure errors still return JSON-RPC errors.

## Tests

Add failing tests first:

1. `tools_list_prints_allowed_and_unclassified_tools` expects `lsp` in `allowed_tools`.
2. MCP `initialize` exposes `resources` capability.
3. `resources/list` includes the three fixed Stateful resources.
4. `resources/read` for `stateful/current` performs `GET /v1/current` and returns its body.
5. `resources/read` rejects an unknown URI.

Then implement the smallest code to pass.
