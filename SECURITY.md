# Security Policy

`stateful_core` is a local coordination layer for coding agents. It is designed
to reduce accidental collisions between trusted local tools. It is not a
sandbox, access-control system, or hard file-locking boundary.

## Supported Scope

Security fixes are handled for the current `v0_main` branch until a stable release
policy exists. The current platform support posture is macOS first. The Rust CI
matrix exercises Linux bubblewrap, but Linux support remains experimental.

## Reporting a Vulnerability

Do not open a public issue with vulnerability details. Use GitHub private
vulnerability reporting for the repository when available. If private reporting
is not enabled and no private contact is listed on the owner profile, open a
minimal public issue asking for a private security channel and do not include
exploit details, affected paths, tokens, logs, or reproduction steps there.

Useful reports include:

- affected command, hook, or HTTP endpoint
- reproduction steps
- expected and actual behavior
- whether the issue requires local shell access
- any generated `.codex`, `.stateful`, or `.stateful_core` files involved

## Local Trust Model

The state server listens only on loopback addresses and rejects non-loopback
listener requests. Non-health HTTP endpoints use a bearer token stored in local
runtime discovery files.

The bearer token establishes a trusted-local coordination domain, not a hard
security boundary. Any token holder can assert task and agent identity; those
identities are not cryptographically separated per task or agent. The token
does not protect against a malicious local user, a compromised shell, or a
process that can read the repository working tree.

## Generated Local Files

The following paths may contain local configuration, runtime state, benchmark
artifacts, absolute paths, or bearer tokens and should not be committed:

- `.codex/`
- `.stateful/`
- `.stateful_core/`
- `.stateful_bench/`

The repository `.gitignore` excludes those paths by default. Public release
archives should be created from Git, for example with `git archive` or a clean
clone, rather than from a working-tree tarball.

## Out of Scope

The project does not currently claim to provide:

- isolation between untrusted local users
- protection from malicious hooks or shell commands
- protection from tools that can modify the repository without using
  `stateful_core`
- distributed locking across machines
- durable secret storage
