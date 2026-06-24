# Contributing

This project is pre-release. Keep changes scoped, documented, and verified against the current implementation contract.

## Development Setup

- Install Rust 1.85 or newer.
- Build with `cargo build --workspace`.
- Run tests with `cargo test --workspace`.
- Run formatting and lint checks with `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings`.

In a repository with stateful hooks enabled, Codex raw Bash is denied and OMP raw Bash is blocked even when it invokes `stateful sandbox run`. Use native read/search tools for ordinary read work. For shell-based read-only inspection, use `stateful sandbox run --fs read-only --network disabled` in Codex or `sandbox_bash` in OMP. For command-shaped repo writes, use `stateful sandbox run --fs write-targets` with exact repo-relative targets plus matching reservation and same-session claim in Codex, or `sandbox_bash` with matching reservation and claim in OMP. For repo-external work, use `stateful sandbox run --fs external --purpose ...` in Codex, `ext_ro_bash` for OMP read-only external commands, and `ext_rw_bash` for OMP external writes with declared scope. Use native edit tools for repo file edits after matching reservation and same-session claim.

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
