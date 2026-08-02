# ADR 0002: Lease-first collision prevention

- **Status:** Accepted
- **Date:** 2026-08-02

## Context

Trusted agent sessions can operate in one local checkout at the same time. A
write decision must account for overlapping physical resources, stale reads, and
an owner that is still performing I/O. Best-effort notices are insufficient:
two writers can observe the same file and both proceed, while immediate retry
can starve a waiting writer.

## Decision

Stateful uses short-lived exclusive lease batches before a write or structured
commit begins.

- A task establishes exact, stable read evidence, then prepares the mutation.
- A physical resource has at most one active exclusive lease in a workspace.
- An agent has at most one active or draining lease batch in a workspace;
  disjoint requests from another task for that agent queue behind the occupied
  slot.
- A task has at most one queued or offered acquisition; later resources join that
  request instead of creating another pending request.
- Conflicting acquisitions queue with a persistent monotonic sequence. Promotion
  respects older conflicting requests, offers the next eligible request, and
  requires a fresh reread plus versioned offer activation.
- An active lease marks each executing write attempt. Release and task ending
  defer deletion while I/O is in flight; completion reports a known result or
  uncertainty before the lease can be released.
- The store records accepted commands, idempotent receipts, and append-only audit
  events in SQLite transactions.

## Consequences

A caller may need to reread or wait before writing, and a completed task can
remain draining until an in-flight operation is resolved. In return, write I/O
never overlaps another holder of the same physical resource, waiting writers
have deterministic ordering, and the audit trail can explain each admission and
transition.

This is local coordination, not a distributed lock service or security control.
Clients that bypass the protocol are outside its protection.
