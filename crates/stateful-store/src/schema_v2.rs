use rusqlite::Connection;

use crate::StoreResult;

pub(crate) fn create_v2_schema(conn: &Connection) -> StoreResult<()> {
    conn.execute_batch(
        "
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS schema_migrations (
            version TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL
        );

        INSERT OR IGNORE INTO schema_migrations (version, applied_at)
        VALUES ('stateful.v2.lease1', '2026-08-02T00:00:00Z');

        CREATE TABLE IF NOT EXISTS tasks (
            workspace_id TEXT NOT NULL,
            task_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('active', 'draining', 'completed', 'failed', 'cancelled')),
            terminal_status TEXT CHECK (terminal_status IN ('completed', 'failed', 'cancelled')),
            next_action TEXT NOT NULL,
            settings_json TEXT NOT NULL,
            handoff TEXT,
            heartbeat_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, task_id)
        );

        CREATE INDEX IF NOT EXISTS idx_tasks_agent_status
            ON tasks(workspace_id, agent_id, status);
        CREATE INDEX IF NOT EXISTS idx_tasks_status_expires
            ON tasks(status, expires_at);

        CREATE TABLE IF NOT EXISTS runtime_processes (
            workspace_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            pid INTEGER NOT NULL,
            process_start_identity TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('active', 'terminated', 'unknown')),
            heartbeat_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, agent_id)
        );

        CREATE TABLE IF NOT EXISTS resources (
            workspace_id TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            kind TEXT NOT NULL CHECK (kind IN ('object', 'entry', 'directory_tree')),
            canonical_path TEXT NOT NULL,
            state_json TEXT NOT NULL,
            generation INTEGER NOT NULL CHECK (generation >= 1),
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, resource_id)
        );

        CREATE TABLE IF NOT EXISTS resource_aliases (
            workspace_id TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            canonical_path TEXT NOT NULL,
            PRIMARY KEY (workspace_id, resource_id, canonical_path),
            FOREIGN KEY (workspace_id, resource_id)
                REFERENCES resources(workspace_id, resource_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS resource_evidence (
            evidence_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            task_id TEXT NOT NULL,
            source_kind TEXT NOT NULL CHECK (source_kind IN ('read', 'own_write')),
            source_id TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            resource_kind TEXT NOT NULL,
            canonical_path TEXT NOT NULL,
            observation_json TEXT NOT NULL,
            generation INTEGER NOT NULL,
            complete INTEGER NOT NULL CHECK (complete IN (0, 1)),
            stable INTEGER NOT NULL CHECK (stable IN (0, 1)),
            exact INTEGER NOT NULL CHECK (exact IN (0, 1)),
            valid INTEGER NOT NULL CHECK (valid IN (0, 1)),
            recorded_at TEXT NOT NULL,
            read_started_at TEXT,
            PRIMARY KEY (evidence_id, resource_id),
            FOREIGN KEY (workspace_id, task_id)
                REFERENCES tasks(workspace_id, task_id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_evidence_task_resource_valid
            ON resource_evidence(workspace_id, task_id, resource_id, valid);

        CREATE TABLE IF NOT EXISTS read_intents (
            workspace_id TEXT NOT NULL,
            task_id TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            resource_kind TEXT NOT NULL,
            canonical_path TEXT NOT NULL,
            evidence_id TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('active', 'revoked', 'released')),
            created_at TEXT NOT NULL,
            read_started_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, task_id, resource_id)
        );

        CREATE INDEX IF NOT EXISTS idx_read_intents_resource_status
            ON read_intents(workspace_id, resource_id, status);

        CREATE TABLE IF NOT EXISTS active_leases (
            batch_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            task_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            mode TEXT NOT NULL CHECK (mode IN ('exclusive_write', 'exclusive_directory')),
            state TEXT NOT NULL CHECK (state IN ('active', 'draining')),
            version INTEGER NOT NULL,
            acquired_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            release_pending INTEGER NOT NULL DEFAULT 0 CHECK (release_pending IN (0, 1)),
            FOREIGN KEY (workspace_id, task_id)
                REFERENCES tasks(workspace_id, task_id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_active_leases_task_state
            ON active_leases(workspace_id, task_id, state);

        CREATE UNIQUE INDEX IF NOT EXISTS idx_unique_active_lease_agent
            ON active_leases(workspace_id, agent_id);

        CREATE TABLE IF NOT EXISTS lease_resources (
            batch_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            resource_kind TEXT NOT NULL,
            canonical_path TEXT NOT NULL,
            acquired_generation INTEGER NOT NULL,
            in_flight_attempt_id TEXT,
            PRIMARY KEY (batch_id, resource_id),
            FOREIGN KEY (batch_id) REFERENCES active_leases(batch_id) ON DELETE CASCADE
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_unique_active_exclusive_resource
            ON lease_resources(workspace_id, resource_id);

        CREATE TABLE IF NOT EXISTS lease_requests (
            batch_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            task_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            mode TEXT NOT NULL CHECK (mode IN ('exclusive_write', 'exclusive_directory')),
            state TEXT NOT NULL CHECK (state IN ('queued', 'offered', 'activated', 'superseded', 'expired', 'cancelled')),
            version INTEGER NOT NULL,
            queue_sequence INTEGER NOT NULL UNIQUE,
            offer_id TEXT,
            offered_at TEXT,
            offer_expires_at TEXT,
            superseded_by TEXT,
            expires_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (workspace_id, task_id)
                REFERENCES tasks(workspace_id, task_id) ON DELETE CASCADE
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_one_nonterminal_request_per_task
            ON lease_requests(workspace_id, task_id)
            WHERE state IN ('queued', 'offered');
        CREATE INDEX IF NOT EXISTS idx_lease_requests_fifo
            ON lease_requests(state, queue_sequence);

        CREATE TABLE IF NOT EXISTS lease_request_resources (
            batch_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            resource_kind TEXT NOT NULL,
            canonical_path TEXT NOT NULL,
            PRIMARY KEY (batch_id, resource_id),
            FOREIGN KEY (batch_id) REFERENCES lease_requests(batch_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS read_attempts (
            read_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            task_id TEXT NOT NULL,
            invocation_id TEXT NOT NULL,
            resources_json TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('started', 'completed', 'failed')),
            started_at TEXT NOT NULL,
            completed_at TEXT,
            UNIQUE (workspace_id, task_id, invocation_id),
            FOREIGN KEY (workspace_id, task_id)
                REFERENCES tasks(workspace_id, task_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS write_attempts (
            attempt_id TEXT PRIMARY KEY,
            permit_id TEXT NOT NULL UNIQUE,
            workspace_id TEXT NOT NULL,
            task_id TEXT NOT NULL,
            invocation_id TEXT NOT NULL,
            batch_id TEXT NOT NULL,
            operation_json TEXT NOT NULL,
            start_observations_json TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('executing', 'completed', 'failed', 'uncertain')),
            started_at TEXT NOT NULL,
            deadline TEXT NOT NULL,
            completed_at TEXT,
            terminal_result_json TEXT,
            UNIQUE (workspace_id, task_id, invocation_id),
            FOREIGN KEY (workspace_id, task_id)
                REFERENCES tasks(workspace_id, task_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS command_events (
            event_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            task_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            request_id TEXT NOT NULL,
            contract_revision TEXT NOT NULL,
            command_kind TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            response_json TEXT NOT NULL,
            recorded_at TEXT NOT NULL,
            UNIQUE (workspace_id, agent_id, request_id)
        );

        CREATE INDEX IF NOT EXISTS idx_command_events_workspace_recorded
            ON command_events(workspace_id, recorded_at);

        CREATE TABLE IF NOT EXISTS command_receipts (
            workspace_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            request_id TEXT NOT NULL,
            contract_revision TEXT NOT NULL,
            command_kind TEXT NOT NULL,
            payload_digest TEXT NOT NULL,
            response_json TEXT NOT NULL,
            recorded_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, agent_id, request_id)
        );

        CREATE TABLE IF NOT EXISTS audit_events (
            event_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            task_id TEXT,
            agent_id TEXT NOT NULL,
            event_type TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_audit_workspace_created
            ON audit_events(workspace_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_audit_created
            ON audit_events(created_at);

        CREATE TABLE IF NOT EXISTS coordination_sequences (
            name TEXT PRIMARY KEY,
            value INTEGER NOT NULL
        );
        INSERT OR IGNORE INTO coordination_sequences(name, value) VALUES ('lease_queue', 0);
        "
    )?;
    Ok(())
}
