# Contributing

This project is pre-release. Keep changes scoped, documented, and verified against the current implementation contract.

## Development Setup

- Install Rust 1.85 or newer.
- Build with `cargo build --workspace`.
- Run tests with `cargo test --workspace`.
- Run formatting and lint checks with `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings`.

In a repository with stateful hooks enabled, raw Bash is denied. Use native read/search tools for ordinary read work, `stateful sandbox run --fs read-only --network disabled` for shell-based read-only inspection, `stateful sandbox run --fs write-targets` with exact targets for command-shaped repo writes, and native Codex edit tools for repo file edits after matching intent and same-session lease.

## Documentation

Behavioral changes should update the relevant contract documents:

- `README.md` for user-facing setup and command guidance.
- `docs/state-model.md` for state and policy semantics.
- `docs/implementation-contract.md` for concrete API, storage, hook, and test contracts.
- `docs/architecture.md` for product-level architecture.

Historical records under `docs/superpowers/` are kept for traceability. Do not treat them as current user-facing guidance.

## Generated Local State

Do not commit generated runtime, integration, or benchmark state. The ignored paths may contain local paths, tokens, runtime databases, or benchmark artifacts:

- `.codex/`
- `.stateful/`
- `.stateful_core/`
- `.stateful_bench/`

Public release archives should be produced from Git, such as with `git archive` or a clean clone.

## Platform Scope

The current release posture is macOS first. The macOS Seatbelt sandbox backend is the verified path. Linux bubblewrap support is implemented but experimental until it is verified in a Linux release environment.

## Security

Do not include vulnerability details in public issues. Follow `SECURITY.md` for private reporting guidance.

## License

By contributing, you agree that your contribution is provided under the GNU Affero General Public License version 3.0 only.
