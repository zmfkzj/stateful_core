# Contributing

This project is pre-release. Keep changes scoped, documented, and verified against the active V2 contracts.

## Development Setup

- Install Rust 1.85 or newer.
- Build with `cargo build --workspace`.
- Run tests with `cargo test --workspace`.
- Run formatting and lint checks with `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings`.

When Stateful hooks are enabled, use native repository tools for reads and edits. Hooks authorize supported mutations through the V2 task, evidence, and lease flow; do not bypass a denial with raw shell writes.

## Documentation

Behavioral changes should update the relevant canonical documents:

- `README.md` for user-facing setup and command guidance.
- `docs/architecture.md` for components, flows, and invariants.
- `docs/usage-reference.md` for CLI, HTTP, and hook contracts.
- `docs/state-model.md` for storage, audit, projection, and migration semantics.

## Generated Local State

Do not commit generated runtime or integration state. Ignored paths may contain local paths, tokens, or runtime databases:

- `.codex/`
- `.stateful/`
- `.stateful_core/`

Public release archives should be produced from Git, such as with `git archive` or a clean clone.

## Platform Scope

The Rust CI matrix exercises both the macOS Seatbelt and Linux bubblewrap sandbox backends. macOS remains the primary release platform; Linux support remains experimental.

## Security

Do not include vulnerability details in public issues. Follow `SECURITY.md` for private reporting guidance.

## License

By contributing, you agree that your contribution is provided under the GNU Affero General Public License version 3.0 only.
