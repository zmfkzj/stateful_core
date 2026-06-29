# OMP Tools

## Use OMP-Native Stateful Tools

- If Stateful MCP tools are missing from the active OMP tool list, use `search_tool_bm25` once with a query such as `stateful state current read`, then call the activated runtime-specific tool names.
- Never fall back to Bash for Stateful coordination.
- OMP built-in Bash may run strict trusted `stateful sandbox run ...` and `stateful sandbox process find ...` commands: bare `stateful` is trusted only after session-start preflight hash-verifies the first PATH `stateful` binary against the installed Stateful binary; commands using the installed absolute binary path remain trusted. Arbitrary raw Bash and Python/JavaScript/JS/Ruby/Julia eval tools are denied.
- Use built-in Bash for sandboxed command execution via a single trusted `stateful sandbox run ...` command with the narrowest valid sandbox profile and any required Stateful reservation/claim preflight.
- Use built-in Bash for process inspection via a single trusted `stateful sandbox process find ...` command.
- External sandbox write/create/write-dir, socket, or signal scope prompts for a scoped OMP UI grant unless the profile sets `stateful.autoApprove: true`.

## Installed OMP Profile

OMP installs the integration into the live OMP profile agent directory, defaulting to `~/.omp/profiles/stateful/agent`, with:

- the Stateful extension,
- `rules/stateful-required.md`,
- `skills/stateful-command-policy/SKILL.md` plus support files,
- `lazy_edit_resume`, `lazy_write_resume`, and `lazy_bash_resume` tools.

The installer merges an existing `config.yml` and rejects invalid YAML. Without `stateful install --agent omp --update`, existing OMP scalar config values are preserved and only missing Stateful keys are inserted. With `--update`, targeted OMP scalar values are overwritten to delegate safety to Stateful hooks while raw Bash/eval denials, Bash passthrough preflight, and sandbox confirmations remain hook-enforced.

## Generated Tool Behavior

- The generated tools are the lazy resume helpers: `lazy_edit_resume`, `lazy_write_resume`, and `lazy_bash_resume`.
- The OMP extension subscribes to Stateful SSE notifications after `session-start`. When a queued reservation is claimable, it injects a next-turn message with the `wait_id`, action/path, and purpose. Agents must still reread the target and call `state_reservation_claim` or rely on an authorized lazy-claim write boundary.
- Blocked OMP `edit` calls with safe repo-relative line targets can be stored as live-session lazy edit operations. Blocked OMP `write` calls can be stored as live-session lazy write operations with captured full write content. Wait-queue denials use the `wait_id`; `missing_reservation` and `missing_claim` denials use a generated operation id. Resume line-based edits with `lazy_edit_resume`; resume captured writes with `lazy_write_resume`, which fails if the target changed since the operation was queued.
- External OMP Bash commands that cannot display the scoped external grant prompt can be stored as live-session lazy bash operations. Resume them with `lazy_bash_resume`, which asks for the same grant, re-authorizes the original Bash tool call, and reruns the stored trusted `stateful sandbox run --fs external ...` command.
- Built-in Bash remains the command execution path for strict trusted `stateful sandbox run ...` and `stateful sandbox process find ...` commands; generated Bash resume exists only for grant-prompt recovery.

## DeNovo OMP Runs

Stateful-bench DeNovo OMP runs use isolated per-instance OMP homes. `stateful:on` conditions install the integration into that isolated home. Docker-backed stateful OMP runs use `/home/stateful` as the container runtime home so the mounted integration path is visible inside the agent. `stateful:off` / no-state conditions may still use an isolated OMP home, but must not be treated as Stateful sessions or repaired with Stateful MCP/sandbox guidance unless the benchmark prompt explicitly enables Stateful.
