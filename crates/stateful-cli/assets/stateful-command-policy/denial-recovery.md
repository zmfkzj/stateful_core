# Denial Recovery

Denials are the API. Read the denial, change the authorization path, and do not retry small command variants.

## Missing Scope Or Claim

If the denial says `missing_reservation`, `missing_claim`, `Target is outside active reservation scope`, or asks for exact scope:

1. Re-read the target if it exists.
2. Add the missing exact file or directory scopes to the task reservation with `state_reservation_declare(purpose=<task purpose>, files_planned=[...])`.
3. Acquire exact same-reservation claims with `state_claim_acquire(reservation_id=<reservation_id>, paths=[...])`.
4. Use native edit tools for repo edits, or `sandbox run --fs write-targets` / OMP built-in Bash with a strict trusted `stateful sandbox run --fs write-targets ...` command and matching targets for command-shaped writes.

Use file claims for file writes, deletes, renames, and moves. Directory claims only authorize directory writes.

## Claim Conflict Or Wait Queue

If `state_claim_acquire` reports `claim_conflict`, do not retry acquisition or steal the claim.

- To wait for a path, call `state_reservation_request` with a stable `request_id`, denied `action`, `path`, and `purpose`.
- Poll `state_notifications_poll` or `state_resume_next` for the reservation.
- When reserved, reread the target.
- Native edits and write-target sandbox writes can lazy-claim the reservation at the next write boundary; manual native-tool/CLI flows should first call `state_reservation_claim(reservation_id=<reservation_id>, wait_id=<wait_id>)` or `stateful reservation claim --reservation-id <reservation_id> --wait-id <wait_id>`.

If a denial already includes `wait_id`, `queue_position`, or reservation guidance, follow that wait queue protocol.

For disposable repo `tmp/` directory claim conflicts, prefer a different session-unique scratch child over waiting when the artifact is truly disposable. Build/test commands should use the build profile's external `/tmp/stateful` scratch instead of repo `tmp/`.

## Raw Bash Or Eval Blocked

If raw Bash or eval-tool execution is blocked:

- Use native Stateful coordination tools and native inspection first.
- Use native edit tools after reservation and claims for repo edits.
- Codex fallback command paths: read-only sandbox for read-only shell inspection, `stateful sandbox process find` for process checks, write-targets sandbox for command-shaped writes, and git profile for git.
- OMP fallback command paths: built-in Bash with strict trusted `stateful sandbox run ...` or `stateful sandbox process find ...` commands. Bare `stateful` is trusted only after session-start preflight hash-verifies the first PATH `stateful` binary against the installed Stateful binary; commands using the installed absolute binary path remain trusted.

Do not wrap `stateful sandbox run` in arbitrary Bash/eval in OMP; only the built-in Bash strict trusted Stateful command path is allowed.

## External Work

Use external only when the command is outside the repo or needs external OS capabilities.

- Codex: `sandbox run --fs external --purpose ... --command ...`.
- OMP: built-in Bash with strict trusted `stateful sandbox run --fs external ...`; write/create/write-dir/socket/signal scope prompts unless `stateful.autoApprove: true`.
- Add absolute external targets, directories, sockets, or signal permission only when needed.
- Repo-relative external write scopes require matching Stateful reservation and same-reservation claims.

## Other Denials

- If a denial mentions a Stateful coordination tool, prefer that active native tool.
- If the running server does not support sandbox write directories, restart with a binary that supports write-directory authorization, then reread and reacquire the needed claim.
- If `run-nested-codex-benchmark` is denied for missing feature support, install or rebuild the trusted binary with the `codex-benchmark` feature before retrying.
- If native Stateful tools are unavailable, do not try legacy `stateful mcp call`; use the active tool list or report the missing tool and denial.
- If no policy-compliant path exists, report the exact command and denial reason.
