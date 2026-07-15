PRAGMA foreign_keys = ON;

CREATE TABLE schema_migrations (
    version TEXT PRIMARY KEY,
    applied_at TEXT NOT NULL
);
INSERT INTO schema_migrations (version, applied_at) VALUES
    ('stateful.v1.initial', '2026-05-31T00:00:00Z');

CREATE TABLE events (
    event_id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    sequence INTEGER,
    repo_id TEXT,
    worktree_id TEXT,
    root TEXT,
    branch TEXT,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_events_workspace_created_at ON events(workspace_id, created_at);

CREATE TABLE agents (
    agent_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE activities (
    activity_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    phase TEXT NOT NULL DEFAULT 'exploring',
    expires_at TEXT
);
CREATE TABLE reservations (
    reservation_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    purpose TEXT NOT NULL,
    scopes_json TEXT NOT NULL,
    status TEXT NOT NULL,
    declared_at TEXT NOT NULL,
    expires_at TEXT
);
CREATE TABLE claims (
    claim_id TEXT PRIMARY KEY,
    reservation_id TEXT,
    agent_id TEXT,
    workspace_id TEXT NOT NULL,
    repo_id TEXT,
    relative_path TEXT,
    absolute_path TEXT,
    purpose TEXT,
    action TEXT NOT NULL DEFAULT 'write_file',
    status TEXT NOT NULL,
    expires_at TEXT,
    observed_exists INTEGER,
    observed_content_hash TEXT
);
CREATE TABLE write_fences (
    fence_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    action TEXT NOT NULL,
    acquired_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    released_at TEXT
);
CREATE TABLE human_observations (
    observation_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    kind TEXT NOT NULL,
    source TEXT NOT NULL,
    confidence TEXT NOT NULL,
    observed_exists INTEGER NOT NULL DEFAULT 1,
    observed_content_hash TEXT,
    observed_at TEXT NOT NULL,
    summary TEXT NOT NULL,
    expires_at TEXT,
    reconciled_at TEXT,
    reconcile_decision TEXT,
    reconciled_by_agent_id TEXT
);
CREATE TABLE wait_queue (
    wait_id TEXT PRIMARY KEY,
    request_id TEXT,
    agent_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    repo_id TEXT,
    worktree_id TEXT,
    root TEXT,
    branch TEXT,
    relative_path TEXT NOT NULL,
    action TEXT NOT NULL,
    status TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    reservation_expires_at TEXT,
    blocking_agent_id TEXT,
    purpose TEXT NOT NULL
);
CREATE TABLE notifications (
    notification_id TEXT PRIMARY KEY,
    sequence INTEGER NOT NULL DEFAULT 0,
    target_agent_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT
);
CREATE TABLE outbox (
    outbox_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL DEFAULT '',
    sequence INTEGER NOT NULL,
    event_type TEXT NOT NULL DEFAULT '',
    payload_json TEXT NOT NULL DEFAULT '{}',
    sync_status TEXT NOT NULL
);

INSERT INTO agents (agent_id, workspace_id, updated_at) VALUES
    ('agent-alpha', 'workspace-main', '2026-07-15T11:00:00Z'),
    ('agent-beta', 'workspace-main', '2026-07-15T11:01:00Z');
INSERT INTO activities (activity_id, agent_id, workspace_id, phase, expires_at) VALUES
    ('activity-alpha-01', 'agent-alpha', 'workspace-main', 'planning', '2026-07-15T12:30:00Z'),
    ('activity-alpha-02', 'agent-alpha', 'workspace-main', 'editing', '2026-07-15T12:30:00Z'),
    ('activity-beta-01', 'agent-beta', 'workspace-main', 'reviewing', '2026-07-15T12:20:00Z');
INSERT INTO reservations (reservation_id, agent_id, workspace_id, purpose, scopes_json, status, declared_at, expires_at) VALUES
    ('reservation-active', 'agent-alpha', 'workspace-main', 'update state machine', '[{"kind":"file","path":"src/state.rs"}]', 'active', '2026-07-15T11:00:00Z', '2026-07-15T12:30:00Z'),
    ('reservation-expired', 'agent-beta', 'workspace-main', 'review state machine', '[{"kind":"file","path":"src/review.rs"}]', 'expired', '2026-07-15T10:00:00Z', '2026-07-15T10:30:00Z');
INSERT INTO claims (claim_id, reservation_id, agent_id, workspace_id, repo_id, relative_path, absolute_path, purpose, action, status, expires_at, observed_exists, observed_content_hash) VALUES
    ('claim-active', 'reservation-active', 'agent-alpha', 'workspace-main', 'repo-main', 'src/state.rs', '/repo/src/state.rs', 'update state machine', 'write_file', 'active', '2026-07-15T12:30:00Z', 1, 'legacy-claim-sha'),
    ('claim-expired', 'reservation-expired', 'agent-beta', 'workspace-main', 'repo-main', 'src/review.rs', '/repo/src/review.rs', 'review state machine', 'write_file', 'expired', '2026-07-15T10:30:00Z', 0, 'expired-claim-sha');
INSERT INTO write_fences (fence_id, agent_id, workspace_id, relative_path, action, acquired_at, expires_at, released_at) VALUES
    ('fence-active', 'agent-alpha', 'workspace-main', 'src/state.rs', 'write_file', '2026-07-15T11:02:00Z', '2026-07-15T12:31:00Z', NULL),
    ('fence-expired', 'agent-beta', 'workspace-main', 'src/review.rs', 'write_file', '2026-07-15T10:02:00Z', '2026-07-15T10:31:00Z', '2026-07-15T10:31:00Z');
INSERT INTO human_observations (observation_id, workspace_id, relative_path, kind, source, confidence, observed_exists, observed_content_hash, observed_at, summary, expires_at, reconciled_at, reconcile_decision, reconciled_by_agent_id) VALUES
    ('human-reconciled', 'workspace-main', 'src/state.rs', 'edit', 'human', 'high', 1, 'human-sha', '2026-07-15T11:03:00Z', 'human changed state', '2026-07-15T13:00:00Z', '2026-07-15T11:04:00Z', 'adopt', 'agent-alpha'),
    ('human-unreconciled', 'workspace-main', 'src/review.rs', 'edit', 'human', 'medium', 0, NULL, '2026-07-15T11:05:00Z', 'human removed review', '2026-07-15T13:00:00Z', NULL, NULL, NULL);
INSERT INTO wait_queue (wait_id, request_id, agent_id, workspace_id, repo_id, worktree_id, root, branch, relative_path, action, status, requested_at, reservation_expires_at, blocking_agent_id, purpose) VALUES
    ('wait-active', 'request-wait-active', 'agent-beta', 'workspace-main', 'repo-main', 'worktree-main', '/repo', 'main', 'src/state.rs', 'write_file', 'waiting', '2026-07-15T11:06:00Z', '2026-07-15T12:30:00Z', 'agent-alpha', 'update state machine'),
    ('wait-expired', 'request-wait-expired', 'agent-beta', 'workspace-main', 'repo-main', 'worktree-main', '/repo', 'main', 'src/review.rs', 'write_file', 'expired', '2026-07-15T10:06:00Z', '2026-07-15T10:30:00Z', 'agent-alpha', 'review state machine'),
    ('wait-offset-late', 'request-wait-offset-late', 'agent-beta', 'workspace-main', 'repo-main', 'worktree-main', '/repo', 'main', 'src/offset-late.rs', 'write_file', 'waiting', '2026-07-15T10:45:00-02:00', '2026-07-15T12:30:00Z', 'agent-alpha', 'late by instant'),
    ('wait-offset-early', 'request-wait-offset-early', 'agent-beta', 'workspace-main', 'repo-main', 'worktree-main', '/repo', 'main', 'src/offset-early.rs', 'write_file', 'waiting', '2026-07-15T11:45:00+02:00', '2026-07-15T12:30:00Z', 'agent-alpha', 'early by instant'),
    ('wait-tie-a', 'request-wait-tie-a', 'agent-beta', 'workspace-main', 'repo-main', 'worktree-main', '/repo', 'main', 'src/tie-a.rs', 'write_file', 'waiting', '2026-07-15T12:00:00Z', '2026-07-15T12:30:00Z', 'agent-alpha', 'tie by instant'),
    ('wait-tie-b', 'request-wait-tie-b', 'agent-beta', 'workspace-main', 'repo-main', 'worktree-main', '/repo', 'main', 'src/tie-b.rs', 'write_file', 'waiting', '2026-07-15T13:00:00+01:00', '2026-07-15T12:30:00Z', 'agent-alpha', 'tie by instant');
INSERT INTO notifications (notification_id, sequence, target_agent_id, workspace_id, kind, payload_json, status, created_at, expires_at) VALUES
    ('notification-pending', 1, 'agent-alpha', 'workspace-main', 'coordination', '{"message":"pending"}', 'pending', '2026-07-15T11:07:00Z', '2026-07-15T13:00:00Z'),
    ('notification-delivered', 2, 'agent-beta', 'workspace-main', 'coordination', '{"message":"delivered"}', 'delivered', '2026-07-15T11:08:00Z', '2026-07-15T13:00:00Z');
INSERT INTO outbox (outbox_id, agent_id, workspace_id, sequence, event_type, payload_json, sync_status) VALUES
    ('outbox-pending', 'agent-alpha', 'workspace-main', 3, 'notify', '{"message":"outbox pending"}', 'pending'),
    ('outbox-delivered', 'agent-beta', 'workspace-main', 4, 'notify', '{"message":"outbox delivered"}', 'delivered');
INSERT INTO events (event_id, event_type, agent_id, workspace_id, sequence, repo_id, worktree_id, root, branch, payload_json, created_at) VALUES
    ('event-z', 'ActivityFinalized', 'agent-alpha', 'workspace-main', 9, 'repo-main', 'worktree-main', '/repo', 'main', '{"cleanup_count":2,"unavailable":"actor"}', '2026-07-15T11:09:00Z'),
    ('event-a', 'AgentRegistered', 'agent-beta', 'workspace-main', 2, 'repo-main', 'worktree-main', '/repo', 'main', '{"legacy":"audit"}', '2026-07-15T11:09:00Z'),
    ('event-b', 'ClaimAcquired', 'agent-alpha', 'workspace-main', 3, 'repo-main', 'worktree-main', '/repo', 'main', '{"legacy":"audit"}', '2026-07-15T11:10:00Z');
