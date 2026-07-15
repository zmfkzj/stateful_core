### Task 9: Cut CLI, Runtime, Outbox, Watcher, and Doctor to V2

**Status:** Task 9 implementation complete; full package verification remains blocked by intentionally out-of-scope V1 hook tests.

#### RED evidence

- `stateful sandbox run --fs build --network disabled --write-dir v2-cli-red --command 'cargo test -p stateful-cli --test runtime --test outbox --test server_lifecycle --test cli'` exited 101 before the migration: V1 reservation requests, legacy outbox records, and enforcement defaults violated the new contracts.
- The focused RED checks observed the required failures: `runtime_post_wraps_typed_payload_in_v2_envelope`, `runtime_get_serializes_full_query_identity`, `unsupported_runtime_protocol_fails_before_mutation`, `recovery_outbox_preserves_original_request_id_and_retries_idempotently`, and `server_start_and_install_default_to_awareness` all failed against V1/default behavior. The watcher V2 test could not compile until the server/store journal diagnostic API was exposed.

#### Implementation

- Centralized CLI V2 request/query construction, flattened full-identity queries, response/error decoding, and runtime identity/schema/capability handshake checks in `runtime.rs`.
- Migrated runtime reservation calls, current/events/resume/human commands, and watcher observation to `/v2/*` envelopes with fresh UUIDs.
- Made outbox persistence retain the serialized original `RequestEnvelope`, route, request UUID, and attempt metadata; replay sends the stored bytes unchanged.
- Set CLI/server/install lifecycle defaults to awareness, retaining explicit enforcement.
- Added sanitized journal diagnostics (size, rows, event types, time range, threshold warning) without mutation, VACUUM, or payload output. Journal inspection lives in `stateful-store`.
- Removed redundant lifecycle double-probing: a V2 identity/capability health check now performs one handshake.

#### GREEN evidence

- `cargo test -p stateful-cli --test runtime`: **24 passed**.
- `cargo test -p stateful-cli --test outbox`: **16 passed**.
- `cargo test -p stateful-cli --test server_lifecycle`: **21 passed**.
- `cargo test -p stateful-cli --test cli`: **56 passed**.
- `cargo test -p stateful-cli`: CLI unit tests (**123 passed**) and Task 9 integration suites passed; package verification then failed in `tests/hook.rs` (**42 failures**) because the intentionally untouched Task 10/11 V1 hook/OMP clients and assertions still target `/v1/*`.

#### Concerns

- Task 10+ hook, OMP, VS Code, benchmark, and documentation behavior was not changed. Their V1 test failures prevent a fully green `stateful-cli` package suite until the subsequent hook migration is applied.
