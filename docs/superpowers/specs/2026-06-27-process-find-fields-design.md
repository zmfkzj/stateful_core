# process_find safe output fields

## Goal

Make `process_find` useful as the approved replacement for raw `ps` by returning all safe process metadata by default while continuing to hide argv, command strings, and environment data from result JSON.

## Current behavior

`stateful sandbox process find` currently reads `ps -axo pid=,ppid=,pgid=,stat=,etime=,pcpu=,comm=,command=`. It uses the full command line internally for `--contains` filtering, but returns only `pid`, `ppid`, `pgid`, `stat`, `etime`, `pcpu`, and `comm`.

## Requirements

- Keep `command`, full argv, and environment variables out of output JSON.
- Keep `--contains` filtering against the internal command line so existing selectors keep working.
- Return richer safe metadata by default.
- Let callers restrict returned fields when they want less output.
- Reject unknown or forbidden output fields with clear errors.

## Design

Extend `SandboxProcessInfo` with safe typed/string fields from `ps`:

- identity/session: `user`, `uid`, `tty`
- timing/state: `stat`, `start`, `etime`, `time`
- CPU/memory: `pcpu`, `pmem`, `rss`, `vsz`
- scheduling: `nice`, `pri`
- existing identifiers: `pid`, `ppid`, `pgid`, `comm`

Add an optional field selector:

- CLI: `stateful sandbox process find ... --field pid --field user`
- OMP tool: `fields: ["pid", "user"]`

When no field selector is provided, return every safe field. When a selector is provided, serialize only those fields. `command`, `args`, `argv`, and `env` are invalid selector names.

## Implementation notes

Use a fixed `ps -axo` format and parse from the left, leaving the command column only in the internal row used for filtering. Serialization should avoid hand-built JSON strings where possible; a map-based serializer for selected fields is acceptable because `process_find` output is small and field selection is user-facing.

## Testing

Add tests before implementation for:

- default output includes the new safe fields
- selected fields omit unselected safe fields
- forbidden fields are rejected
- `contains` still matches the internal command string without exposing it

## Documentation

Update OMP tool schema text and user docs to mention `fields` and the safe default field set.
