# Changelog

All notable changes to `stateful_core` will be documented in this file.

The project is pre-release. Until a stable release process exists, changes are tracked under `Unreleased`.

## Unreleased

### Added

- Added mandatory intent purpose guidance across CLI, MCP, HTTP, docs, and hook policy.
- Added explicit write reservation, claim, cancellation, notification, and resume surfaces.
- Added store-backed current-state context rendering for active intents, leases, and queued or reserved wait records.
- Added structured commit and push wrappers.
- Added sandbox-run support for read-only shell inspection and declared-target command-shaped writes.

### Changed

- Intent declaration now requires at least one non-empty `files_planned` entry, and intent request now requires a non-empty `path`; empty or normalized-empty paths are rejected.
- Session heartbeats refresh active leases, activity records, and active intent expiry, with intent lifetime capped at 60 minutes from declaration.
- Wait queue reservation claim state is represented as `claimed`.
- Public documentation now presents macOS as the first verified platform and Linux bubblewrap support as experimental.

### Security

- Documented local trust boundaries, generated local state risks, and public release archive guidance.
