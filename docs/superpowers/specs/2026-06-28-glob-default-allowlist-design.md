# glob default allowlist

## Goal

Make `glob` a default allowed OMP tool so repo-enabled sessions do not treat routine file discovery as an unclassified tool.

## Current behavior

Default repo tool allowlists are assembled in `crates/stateful-cli/src/repo_registry.rs` from Codex-specific tools and OMP-specific tools. The OMP default currently includes `task`, `yield`, and `lsp`, but not `glob`.

When an OMP session uses `glob` in a repo that relies on the default allowlist, the hook can classify it as unclassified instead of allowing it through the repo allowlist path.

## Requirements

- Add `glob` to the OMP default allowed tools.
- Preserve the existing Codex default allowed tools unchanged.
- Preserve user-managed allowlist behavior: repo-scoped additions, removals, deduplication, and re-enable preservation.
- Update tests that assert the exact default allowlist order.
- Do not add a separate documentation list of every default tool; existing docs already describe the default allowlist source.

## Design

Append `"glob"` to `DEFAULT_OMP_ALLOWED_TOOLS` in `crates/stateful-cli/src/repo_registry.rs`.

Update exact expected allowlist helpers/assertions in:

- `crates/stateful-cli/tests/repo_registry.rs`
- `crates/stateful-cli/tests/cli.rs`

No new abstraction is needed. The default allowlist builder already preserves existing user allowlist entries and deduplicates defaults through the existing `default_allowed_tools_with_existing` path.

## Testing

Use TDD for the behavior change:

1. Add `glob` to default allowlist expectations first.
2. Run the targeted repo registry and CLI tests and verify they fail because production defaults do not include `glob` yet.
3. Add `glob` to `DEFAULT_OMP_ALLOWED_TOOLS`.
4. Re-run the same targeted tests and verify they pass.

## Documentation

No README or usage-reference change is required because the docs intentionally describe default allowlists by source, not by enumerating every entry.
