# Stateful Sandbox Run Design

## Context

`stateful_core` is moving toward a centralized state server for intent,
leases, queues, and authorization, while keeping local filesystem mutation on
the local machine. The existing `state_bash_write` MCP tool breaks that
boundary because it is an MCP surface that performs local file writes through a
CLI-local bridge.

The replacement design removes local file writes from MCP and introduces one
local command runner:

```text
stateful sandbox run --fs <profile> --network <policy> --command <command>
```

The runner is a coordination-aware local filesystem write-scope wrapper for
trusted agent tooling. It is not a general access-control system and does not
claim to control network side effects beyond the selected network sandbox
policy.

## Goals

- Remove `state_bash_write` / `state.bash.write` from the MCP tool surface.
- Keep intent, lease, conflict, notification, and resume operations centralized
  in the state server and exposed through MCP/HTTP.
- Route shell command execution through a local CLI wrapper that can enforce
  filesystem profiles and call `/v1/authorize` before writes.
- Use one generic runner instead of growing command-specific wrappers for
  every tool.
- Preserve practical shell behavior by making `/dev/null` writable in every
  filesystem profile.

## Non-Goals

- Do not add a Codex CLI `exec_command` schema extension in this change.
- Do not support command-string metadata markers.
- Do not support `-- <argv...>` execution in the MVP.
- Do not ship generic `git-metadata` or `workspace` write profiles in the MVP.
- Do not make `stateful` responsible for content provenance, read
  confidentiality, or all possible network side effects.

## CLI Contract

MVP syntax:

```text
stateful sandbox run
  --fs read-only|write-targets
  --network disabled|enabled
  --write-target <repo-relative-path>...
  --create-target <repo-relative-path>...
  --command <single string>
  [--timeout-seconds <seconds>]
```

Defaults:

- `--fs read-only` when omitted.
- `--network disabled` when omitted.
- `--timeout-seconds 300` when omitted.

`--command` is required and must appear exactly once. The inner command is an
opaque string executed inside the selected sandbox with `/bin/sh -c`.

`read-only` profile:

- Rejects `--write-target` and `--create-target`.
- Does not call `/v1/authorize`.
- Denies repo/source-tree writes at the sandbox layer.
- Allows `/dev/null` writes.

`write-targets` profile:

- Requires at least one `--write-target` or `--create-target`.
- Requires every target to be repo-relative.
- Rejects empty paths, absolute paths, `..`, `.git`, control characters,
  symlink file targets, symlinked parent directories, and directory targets.
- Calls `/v1/authorize` for each write/create target before execution.
- Runs the command with only authorized targets writable.
- Allows `/dev/null` writes.

`create-target` pre-creation:

- The wrapper may create missing parent directories and files before entering
  the sandbox, after authorization and safety checks.
- This pre-creation is a trusted wrapper side effect, not an inner command side
  effect.
- Parent directory creation is limited to safe repo-local non-symlink paths.

Network policy:

- `--network disabled` runs with network disabled where the platform backend
  supports it.
- `--network enabled` allows network access for the sandboxed command.
- `stateful` still only guarantees local filesystem write scope and
  coordination. Network side effects, external content provenance, and
  exfiltration are outside the runner's guarantee.

## Session Binding

`stateful sandbox run` is session-bound in Codex usage.

- The wrapper reads the current stateful session file, including run-bound
  session files when `STATEFUL_CODEX_RUN_ID` is present.
- If no current session exists, `write-targets` fails closed.
- The MVP does not accept user-supplied `--session-id` or `--workspace-id` for
  sandbox writes. This avoids session spoofing through Bash command text.
- `read-only` may run without a current session, but hook-allowed Codex usage
  still records the active session through lifecycle hooks.

## Authorization Flow

For `write-targets`, the wrapper builds one `/v1/authorize` request per target.

Request properties:

- `source.kind`: `cli`
- `source.event`: `sandbox_run`
- `source.tool_name`: `stateful.sandbox.run`
- `payload.action`: `write_file`
- `payload.path`: normalized repo-relative target
- `payload.queue_on_conflict`: `true`
- `payload.fs_profile`: `write-targets`
- `payload.network_policy`: `disabled` or `enabled`

If any target is denied, the inner command is not executed. The response
includes allowed and denied targets. Non-2xx server errors fail closed for
`write-targets`.

## Hook Contract

Codex `PreToolUse` for `Bash` no longer authorizes raw command text directly.
It allows only strict wrapper invocations.

Allowed outer command shape:

- Exactly one simple command.
- Executable is the trusted `stateful` binary.
- Arguments begin with `sandbox run`.
- Exactly one `--command <single string>` argument.
- No outer redirects.
- No outer pipelines.
- No `;`, `&&`, `||`, newline command separation, subshells, background jobs,
  process substitution, or command substitution.
- No outer environment assignments.
- No command prefixes such as `sudo`, `env`, `time`, or shell aliases.
- No `-- <argv...>` mode.
- No command-string metadata marker mode.

Executable identity:

- Prefer an absolute canonical executable path.
- If a bare `stateful` is accepted, the hook must resolve it and compare the
  canonical path to the installed/current trusted stateful binary before
  allowing execution.
- Path spoofing through the workspace, shell functions, aliases, or modified
  `PATH` must fail closed.

Hook validation is intentionally shallow with respect to the inner command. The
hook validates the wrapper form, fs profile, network policy, target syntax, and
wrapper identity. The wrapper performs target normalization, authorization,
pre-creation, sandbox construction, timeout handling, and command execution.

Raw examples denied by the hook:

```text
rg auth src
git status --short
cargo test
stateful sandbox run --fs read-only --command "rg auth src"; rm README.md
stateful sandbox run --fs write-targets --command "printf x > README.md"
```

Allowed examples:

```text
/trusted/path/stateful sandbox run --fs read-only --network disabled --command "rg auth src"
/trusted/path/stateful sandbox run --fs write-targets --network enabled --write-target README.md --command "printf x > README.md"
```

## Sandbox Backends

The current macOS Seatbelt and Linux bubblewrap code should be extracted from
the existing MCP-local bash bridge into a reusable sandbox module.

Backend requirements:

- `/dev/null` is writable in every profile.
- `read-only` blocks repo/source writes.
- `write-targets` allows exact authorized target file writes.
- `write-targets` blocks unlisted repo writes.
- `--network disabled` disables network where supported.
- `--network enabled` omits the no-network restriction where supported.
- Nonzero inner command exit codes return a command result instead of becoming
  wrapper setup errors.

Linux support must explicitly handle `/dev/null` in bubblewrap and must test
`--network enabled|disabled` behavior separately from macOS.

## MCP Migration

Remove `state_bash_write` / `state.bash.write` from the MCP descriptors and
from `tools/list`.

Most MCP tools continue to map directly to HTTP:

- session register/heartbeat
- intent declare
- lease acquire/release
- conflicts check
- reconcile ack
- notifications poll
- resume next

`state_bash_write` has no compatibility alias. Direct stale calls should return
a clear error:

```text
state_bash_write was removed; use `stateful sandbox run ... --command ...`.
```

Documentation, generated installed skill text, README examples, and hook denial
messages must all point to `stateful sandbox run`.

## Deferred Profiles

`git-metadata` is deferred because generic `.git` write access can bypass
structured commit safeguards, stage unrelated user changes, mutate refs, or
run Git hooks outside explicit source-path authorization.

`workspace` is deferred because broad workspace writes need a server-side policy
and audit contract before they can be safely exposed.

Both profiles must fail closed in hooks and in the wrapper until their exact
authorization payloads and sandbox behavior are specified.

Existing structured wrappers such as `stateful commit` and `stateful push` may
remain while these profiles are deferred. They should not expand into a large
family of command-specific wrappers; they are temporary or narrow structured
paths until the generic profile model is precise enough.

## Testing

CLI parsing tests:

- `sandbox run` requires exactly one `--command`.
- `read-only` rejects write/create targets.
- `write-targets` requires at least one write/create target.
- `--network disabled|enabled` parses and defaults to disabled.
- `git-metadata` and `workspace` fail closed.
- `-- <argv...>` and command-string metadata markers are rejected.

Hook tests:

- Raw `rg`, `git status`, `git push`, and `cargo test` are denied.
- Valid canonical `stateful sandbox run --fs read-only ...` is allowed.
- Valid canonical `stateful sandbox run --fs write-targets --write-target ...`
  is allowed.
- Bare `stateful` is rejected unless canonical resolution proves it is the
  trusted binary.
- Outer redirects, pipelines, command separators, env assignments, command
  substitution, aliases, and trailing commands are denied.
- Missing write targets for `write-targets` are denied.
- Unknown or deferred profiles are denied.

Sandbox execution tests:

- Read-only profile blocks repo writes.
- Write-targets profile allows declared file writes.
- Write-targets profile blocks undeclared file writes.
- Create-target creates only declared safe repo-local files.
- `/dev/null` writes work in every profile.
- Network enabled/disabled behavior is covered per backend where available.
- Nonzero inner command exits produce structured command results.

Migration tests:

- MCP `tools/list` does not include `state_bash_write`.
- Stale `state_bash_write` calls produce the removal guidance.
- README, architecture docs, implementation contract, and generated skill text
  no longer recommend `state_bash_write`.

