# Recovery From Warnings And Denials

Denials are the API; awareness warnings use the same response contract. Read the response, change the coordination path, and do not retry small command variants.

## Advisory Scope Or Claim Warning

In awareness mode, missing reservation, missing claim, scope, and claim-conflict results are coordination warnings. Treat them as active-work evidence: inspect rendered context and narrow the plan before writing. In explicit enforcement they can deny.

1. For OMP native `edit`/`write` without an explicit `reservation_id`, allow the extension to auto-declare the exact tool-visible file scope, acquire same-reservation claims, and retry authorization when this is the only issue.
2. Otherwise reread the target if it exists.
3. If the active tool list exposes them, add exact file or directory scopes with `state_reservation_declare(purpose=<task purpose>, files_planned=[...])`, then acquire exact same-reservation claims with `state_claim_acquire(reservation_id=<reservation_id>, paths=[...])`.
4. If those tools are absent, use OMP native `edit`/`write` auto-declare, lazy resume, or an already-authorized `reservation_id` write boundary. Do not invent tools or repair the session through a shell.
5. Use native edit tools for repo edits, or `sandbox run --fs write-targets` / OMP built-in Bash with a strict trusted `stateful sandbox run --fs write-targets ...` command and matching targets for command-shaped writes.

Exact file claims cover file writes, deletes, renames, and moves. Directory claims cover only directory writes.

## Freshness Hard Stops

`unknown_write_outcome`, `stale_observation`, and `write_fence_conflict` deny in both modes. A missing or expired read is a warning in awareness and a denial in explicit enforcement.

1. For `unknown_write_outcome`, complete a matching exact reread and reconcile that write intent before another write.
2. For `stale_observation`, reread every affected exact path, including both source and destination for moves or renames, then regenerate the edit from the current contents.
3. For `write_fence_conflict`, wait for the in-flight write to complete, then reread before retrying.
4. If a freshness issue repeats, stop and report the path and response instead of forcing the write.

## Unreconciled Human Write

`unreconciled_human_write` is a hard stop in both modes: it records a high-confidence human change, not advisory activity.

1. Reread every denied file path.
2. Summarize the human change and decide whether to `adopt`, `reapply`, `ask_user`, or `abandon`.
3. If the active tool list exposes `state_reconcile_ack`, `state.reconcile.ack`, or an MCP-prefixed equivalent, call the exact shown tool with `resource`/`resources`, `reservation_id`, `files_reread`, `summary` or `human_change_summary`, and `decision`.
4. If no reconcile-ack tool is exposed, report the missing native path and do not overwrite.
5. Retry the original write only after an `adopt` or `reapply` acknowledgement succeeds.

## Claim Conflict Or Wait Queue

In awareness, `coordination_conflict` is a warning: inspect rendered context and narrow the plan. It does not create a wait queue or lazy replay operation.

In explicit enforcement, if `state_claim_acquire` reports `coordination_conflict`, do not retry acquisition or steal the claim.

- To wait for a path, call active `state_reservation_request` with a stable `request_id`, denied `action`, `path`, and `purpose`.
- Use the next-turn notification or active `state_notifications_poll` to learn when the reservation is claimable; use active `state_resume_next` as durable recovery for still-active claimable reservations.
- When reserved, reread the target.
- Queued OMP lazy operations resume with `lazy_edit_resume` or `lazy_write_resume` only after the matching `reservation_granted` notification marks them claimable. The helper reauthorizes the original write with stale-target guards; it does not claim the reservation itself.

If a response already includes `wait_id`, `queue_position`, or reservation guidance, follow that wait queue protocol.

For disposable repo `tmp/` directory claim conflicts, prefer a different session-unique scratch child over waiting when the artifact is truly disposable. Build/test commands should use the build profile's external `/tmp/stateful` scratch instead of repo `tmp/`.

## Raw Bash Or Eval Blocked

If raw Bash or eval-tool execution is blocked:

- Use native Stateful coordination tools and native inspection first.
- Use OMP native `edit`/`write` auto-declare/claim for the simple-write path; for other repo edits, use active reservation and claim tools or report the missing path.
- Codex fallback command paths: read-only sandbox for shell inspection, `stateful sandbox process find` for process checks, write-targets sandbox for command-shaped writes, and git profile for git.
- OMP fallback command paths: built-in Bash with strict trusted `stateful sandbox run ...` or `stateful sandbox process find ...` commands. Bare `stateful` is trusted only after session-start or per-tool preflight hash-verifies the first PATH `stateful` binary against the installed Stateful binary; commands using the installed absolute binary path remain trusted.

Do not wrap `stateful sandbox run` in arbitrary Bash/eval in OMP; only the built-in Bash strict trusted Stateful command path is allowed.

## External Work

Use external only when the command is outside the repo or needs external OS capabilities.

- Codex: `sandbox run --fs external --purpose ... --command ...`.
- OMP: built-in Bash with strict trusted `stateful sandbox run --fs external ...`; write/create/write-dir/socket/signal scope and repo-external native `edit`/`write` file targets auto-approve by default through `stateful.autoApprove: true` and prompt only when `stateful.autoApprove: false` is configured.
- Add absolute external targets, directories, sockets, or signal permission only when needed.
- Repo-relative external write scopes require matching Stateful reservation and same-reservation claims.

## Other Denials

- If a denial mentions a Stateful coordination tool, prefer that active native tool.
- If the running server does not support sandbox write directories, restart with a binary that supports write-directory authorization, then reread and reacquire the needed claim.
- If `run-nested-codex-benchmark` is denied for missing feature support, install or rebuild the trusted binary with the `codex-benchmark` feature before retrying.
- If native Stateful tools are unavailable, do not try legacy `stateful mcp call`; use the active tool list or report the missing tool and denial.
- If no policy-compliant path exists, report the exact command and denial reason.
