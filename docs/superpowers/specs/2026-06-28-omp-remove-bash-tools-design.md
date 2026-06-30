# OMP Generated Bash Tool Removal Design

## Goal

Remove Stateful's generated OMP Bash/process custom tools and make OMP use its built-in Bash UX for command-shaped Stateful sandbox operations.

## Problem

Stateful currently installs both:

- OMP built-in Bash passthrough guarded by Stateful preflight, and
- generated custom tools: `sandbox_bash`, `ext_ro_bash`, `ext_rw_bash`, `process_find`, and `sandbox_job_poll`.

This duplicates user-facing command UX. The preferred UX is OMP's original built-in Bash tool, with Stateful accepting only strict trusted `stateful sandbox run ...` and `stateful sandbox process find ...` commands.

## Scope

Remove the generated OMP command tools completely:

- `sandbox_bash`
- `ext_ro_bash`
- `ext_rw_bash`
- `process_find`
- `sandbox_job_poll`

Keep these generated OMP tools:

- `lazy_edit_resume`
- `lazy_write_resume`

Keep the built-in Bash preflight gate and hook authorization for:

- `stateful sandbox run ...`
- `stateful sandbox process find ...`

## Behavior

### Built-in Bash

OMP built-in Bash remains enabled in the Stateful profile.

Allowed commands must be a single strict trusted Stateful invocation:

```text
<absolute-stateful-binary> sandbox run ... --command <cmd>
<absolute-stateful-binary> sandbox process find ...
```

The extension rejects shell control syntax, nested shell tricks, untrusted Stateful paths, and unsupported outer arguments before OMP executes the command.

### External operations

For built-in Bash `stateful sandbox run --fs external ...`:

- read/no declared write scope: no OMP UI prompt
- write/create/write-dir/socket/signal scope: OMP UI grant prompt by default
- `stateful.autoApprove: true`: skips only the Stateful-owned external grant prompt

There is no per-call `auto_approve` flag for built-in Bash passthrough. Adding a wrapper-only flag would make the visible shell command diverge from the real Stateful CLI command, so this design intentionally avoids command rewriting.

### Process inspection

Generated `process_find` is removed. Agents use built-in Bash with:

```text
<absolute-stateful-binary> sandbox process find <selector>
```

The same trusted-binary and request validation remains in the hook.

### Background jobs

Generated sandbox background jobs are removed with the generated sandbox tools. `sandbox_job_poll` is removed because no remaining generated tool produces sandbox job IDs.

Built-in Bash output, PTY, cancellation, and artifact behavior are owned by OMP's native Bash runtime.

### Lazy write recovery

`lazy_edit_resume` and `lazy_write_resume` remain. They are not command-execution tools and are still needed for Stateful reservation/claim recovery.

## Code changes

- `crates/stateful-cli/src/install.rs`
  - Remove registration for `sandbox_bash`, `ext_ro_bash`, `ext_rw_bash`, `process_find`, and `sandbox_job_poll`.
  - Remove helper functions used only by those tools: sandbox arg builders, background sandbox job storage/polling, sandbox stdout streaming, and disabled-sandbox fallback for generated `sandbox_bash`.
  - Keep helpers used by built-in Bash passthrough: strict Stateful command word parsing, `statefulBashPassthroughDecision`, external grant descriptor/settings/storage, and `ensureExternalBashGrant`.
  - Keep lazy resume tool registration and hook event handlers.

- `crates/stateful-cli/src/hook.rs`
  - Keep built-in Bash allow/block behavior for trusted `stateful sandbox run ...` and `stateful sandbox process find ...`.
  - Remove OMP allowlist behavior for the removed generated tools.
  - Update denial text to point to built-in Bash Stateful commands, not generated tools.

- `crates/stateful-cli/tests/hook.rs`
  - Delete tests asserting generated sandbox/process tools are allowed.
  - Add/keep tests asserting built-in Bash trusted sandbox/process commands are allowed and unsafe Bash/eval remains denied.

- `crates/stateful-cli/tests/install_global.rs`
  - Update extension-generation assertions to require no removed tool registrations.
  - Keep assertions for built-in Bash enabled, lazy resume tools, external prompt helpers, and Stateful hook event handlers.

- Documentation and installed skill assets
  - Replace generated tool guidance with built-in Bash guidance.
  - Preserve external grant semantics and `stateful.autoApprove` behavior.
  - Remove `sandbox_job_poll` usage from OMP docs.

## Tests

Use TDD for behavior changes:

1. Add/adjust failing install-generation tests proving removed tools are absent.
2. Add/adjust failing hook tests proving removed generated tools are denied/unclassified and built-in Bash remains allowed for trusted Stateful commands.
3. Implement minimal code deletion/update.
4. Run targeted tests:

```text
cargo test -p stateful-cli --test install_global install_omp_yes_creates_extension_and_mcp_config -- --nocapture
cargo test -p stateful-cli --test hook bash_allows -- --nocapture
cargo test -p stateful-cli --test hook -- --nocapture
cargo fmt --all --check
```

## Non-goals

- Do not add a per-call `auto_approve` syntax to built-in Bash.
- Do not remove lazy edit/write resume tools.
- Do not change Codex command policy.
- Do not add new dependencies.
