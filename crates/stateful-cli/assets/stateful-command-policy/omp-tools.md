# OMP Tools

## Use OMP-Native Stateful Tools

- If Stateful native tools are missing from the active OMP tool list and a tool-discovery tool such as `search_tool_bm25` is exposed, use it once with a query such as `stateful state current read`, then call the activated runtime-specific tool names. If no discovery tool is exposed, use the active tool list plus the documented auto-declare, lazy-resume, or write-boundary path; do not name nonexistent tools as guaranteed.
- If a native Stateful call quickly reports a stale, unreachable, or timed-out OMP runtime connection, treat that as an unavailable authorization runtime. Do not loop retries or repair through Bash/session probes; use one active-tool discovery attempt when available, then the documented auto-declare, lazy-resume, or write-boundary path, or report the unavailable runtime and denial.
- Never fall back to Bash for Stateful coordination.
- OMP built-in Bash may run strict trusted `stateful sandbox run ...` and `stateful sandbox process find ...` commands: bare `stateful` is trusted only after session-start or per-tool preflight hash-verifies the first PATH `stateful` binary against the installed Stateful binary; commands using the installed absolute binary path remain trusted. Arbitrary raw Bash and Python/JavaScript/JS/Ruby/Julia eval tools are denied.
- Use built-in Bash for sandboxed command execution via a single trusted `stateful sandbox run ...` command with one or more `--command <cmd>` flags, the narrowest valid sandbox profile, and any required Stateful reservation/claim preflight. Exact repo file targets on the external profile can auto-declare/claim; repo directory writes still need explicit coordination.
- Use built-in Bash for process inspection via a single trusted `stateful sandbox process find ...` command.
- External sandbox write/create/write-dir, socket, or signal scope and repo-external native `edit`/`write` file targets auto-approve the scoped OMP UI grant by default through `stateful.autoApprove: true`; set `stateful.autoApprove: false` to require the prompt.

## OMP Agent Identity

- OMP derives the active Stateful `agent_id` from `ctx.sessionManager.getSessionId()` only.
- The generated id is `omp-${sessionId}` and stays stable for the whole session, so reservations, claims, and streamed notifications all target the same identity. Session leaf/branch ids are never part of the identity.
- If `getSessionId()` is unavailable or invalid, OMP Stateful actions fail closed. Do not repair identity through event fields, context fields, environment variables, or current-session shell probes.

## Installed OMP Profile

OMP installs the integration into the live OMP profile agent directory, defaulting to `~/.omp/profiles/stateful/agent`, with:

- the Stateful extension,
- `rules/stateful-required.md`,
- `skills/stateful-command-policy/SKILL.md` plus support files,
- `lazy_edit_resume`, `lazy_write_resume`, and `lazy_bash_resume` tools.

The installer merges an existing `config.yml` and rejects invalid YAML. Without `stateful install --agent omp --update`, existing OMP scalar config values are preserved and only missing Stateful keys are inserted. With `--update`, targeted OMP scalar values are overwritten to delegate safety to Stateful hooks while raw Bash/eval denials, Bash passthrough preflight, and sandbox confirmations remain hook-enforced.

## Generated Tool Behavior

- The generated tools are the lazy resume helpers: `lazy_edit_resume`, `lazy_write_resume`, and `lazy_bash_resume`.
- The OMP extension subscribes to sequenced, replayable Stateful SSE notifications after `session-start`, so reconnects can replay missed notices. The stream carries `reservation_granted` for claimable queued reservations and `scope_overlap` when a peer declares overlapping scope. Agents act on `reservation_granted` next-turn messages with the `wait_id`, action/path, and purpose by rereading the target, then resuming the saved lazy operation or retrying the authorized write boundary. Manual reservation claim belongs only to tool contexts that expose `state_reservation_claim` or explicitly permit `stateful reservation claim`.
- `scope_overlap` is advisory, carries no `required_next_action`, and OMP surfaces it as a next-turn FYI without triggering a turn. Coordinate with the peer or adjust the file split if needed; do not redeclare or steal claims in response.
- For native OMP `edit` and `write`, pre-tool authorization uses predeclare/claim as the default simple-write path when no explicit reservation id was supplied and the tool-visible target is one simple repo file: it declares the exact file scope and acquires same-reservation claims before the first authorization. Lazy resume remains the fallback for queued/conflicting operations, stale-target replay, missing base observation, unavailable authorization runtime, unsupported targets, explicit bad reservation ids, and other denials.
- Resume queued/conflicting line-based edits with `lazy_edit_resume`; resume captured writes with `lazy_write_resume`. When the stored lazy operation has a `wait_id`, the resume helper waits/polls `stateful resume next` or notifications for that saved `wait_id` before claiming, then re-authorizes the original tool call and applies with stale-target guards. Generated no-wait lazy operation ids still require resolving the missing scope or claim externally before resume.
- External OMP Bash commands that cannot display the scoped external grant prompt can be stored as live-session lazy bash operations. Resume them with `lazy_bash_resume`, which asks for the same grant, re-authorizes the original Bash tool call, and reruns the stored trusted `stateful sandbox run --fs external ...` command.
- Built-in Bash remains the command execution path for strict trusted `stateful sandbox run ...` and `stateful sandbox process find ...` commands; generated Bash resume exists only for grant-prompt recovery.

## DeNovo OMP Runs

Stateful-bench DeNovo OMP runs use isolated per-instance OMP homes. `stateful:on` conditions install the integration into that isolated home. Docker-backed stateful OMP runs use `/home/stateful` as the container runtime home so the mounted integration path is visible inside the agent. `stateful:off` / no-state conditions may still use an isolated OMP home, but must not be treated as Stateful sessions or repaired with Stateful native-tool/sandbox guidance unless the benchmark prompt explicitly enables Stateful.
