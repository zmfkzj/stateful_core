use rusqlite::Connection;
use serde::Serialize;
use stateful_core::{
    ActorType, AgentIdentity, RequestEnvelope, ReservationScope, SourceKind, SourceRef,
    WorkspaceIdentity,
};
use stateful_store::{FixedClock, ReservationRelease, Store};
use tempfile::TempDir;
use time::{OffsetDateTime, macros::datetime};
use uuid::Uuid;

const FIXTURE: &str = include_str!("fixtures/v1_persistent_state.sql");

const MIGRATION_TIME: OffsetDateTime = datetime!(2026-07-15 11:30 UTC);

fn request<T: Serialize>(agent_id: &str, payload: T) -> RequestEnvelope<T> {
    RequestEnvelope::new(
        Uuid::new_v4(),
        MIGRATION_TIME,
        AgentIdentity {
            agent_id: agent_id.into(),
            turn_id: Some("task9-migration".into()),
            actor_id: format!("{agent_id}-actor"),
            actor_type: ActorType::Agent,
            owner_id: None,
            parent_agent_id: None,
            parent_actor_id: None,
        },
        WorkspaceIdentity {
            root: "/repo".into(),
            workspace_id: "workspace-main".into(),
            repo_id: "repo-main".into(),
            worktree_id: "worktree-main".into(),
            branch: "main".into(),
        },
        SourceRef {
            kind: SourceKind::Cli,
            event: "test".into(),
            tool_name: None,
            source_ref: "task9-migration-authority".into(),
        },
        payload,
    )
    .expect("request should be valid")
}

#[test]
fn migration_preserves_all_reservation_and_claim_scopes_with_null_expiries() {
    let temp = TempDir::new().expect("temporary directory exists");
    let path = temp.path().join("legacy.sqlite");
    let connection = Connection::open(&path).expect("legacy database opens");
    connection.execute_batch(FIXTURE).expect("fixture applies");
    connection
        .execute_batch(
            "
            INSERT INTO reservations (reservation_id, agent_id, workspace_id, purpose, scopes_json, status, declared_at, expires_at)
            VALUES (
                'reservation-no-deadline',
                'agent-alpha',
                'workspace-main',
                'preserve all scopes',
                '[{\"kind\":\"file\",\"path\":\"src/a.rs\"},{\"kind\":\"file\",\"path\":\"src/b.rs\"}]',
                'active',
                '2026-07-15T11:00:00Z',
                NULL
            );
            INSERT INTO claims (claim_id, reservation_id, agent_id, workspace_id, repo_id, relative_path, absolute_path, purpose, action, status, expires_at, observed_exists, observed_content_hash)
            VALUES (
                'claim-no-deadline',
                'reservation-no-deadline',
                'agent-alpha',
                'workspace-main',
                'repo-main',
                'src/c.rs',
                '/repo/src/c.rs',
                'preserve all scopes',
                'write_file',
                'active',
                NULL,
                1,
                NULL
            );
            INSERT INTO claims (claim_id, reservation_id, agent_id, workspace_id, repo_id, relative_path, absolute_path, purpose, action, status, expires_at, observed_exists, observed_content_hash)
            VALUES (
                'claim-no-deadline-directory',
                'reservation-no-deadline',
                'agent-alpha',
                'workspace-main',
                'repo-main',
                'src/directory/',
                '/repo/src/directory',
                'preserve all scopes',
                'write_directory',
                'active',
                NULL,
                1,
                NULL
            );
            ",
        )
        .expect("legacy rows insert");
    drop(connection);

    let mut store = Store::open_with_clock(&path, FixedClock::new(datetime!(2026-07-15 11:30 UTC)))
        .expect("legacy database migrates");

    let reservation = store
        .reservation("workspace-main", "reservation-no-deadline")
        .expect("reservation reads")
        .expect("reservation exists");
    assert_eq!(reservation.status, "active");
    assert_eq!(reservation.expires_at, None);
    assert_eq!(reservation.max_expires_at, None);
    assert_eq!(
        reservation.scopes,
        vec![
            ReservationScope::directory("src/directory"),
            ReservationScope::file("src/a.rs"),
            ReservationScope::file("src/b.rs"),
            ReservationScope::file("src/c.rs"),
        ],
    );

    let claim = store
        .claim("workspace-main", "claim-no-deadline")
        .expect("claim reads")
        .expect("claim exists");
    assert_eq!(claim.status, "active");
    assert_eq!(claim.expires_at, None);

    store
        .rebuild_projections()
        .expect("migration replay succeeds");
    assert_eq!(
        store
            .reservation("workspace-main", "reservation-no-deadline")
            .expect("reservation reads")
            .expect("reservation exists")
            .scopes
            .len(),
        4,
    );
}

#[test]
fn migration_excludes_terminal_claim_scopes_and_preserves_active_claim_scopes() {
    let temp = TempDir::new().expect("temporary directory exists");
    let path = temp.path().join("legacy.sqlite");
    let connection = Connection::open(&path).expect("legacy database opens");
    connection.execute_batch(FIXTURE).expect("fixture applies");
    connection
        .execute_batch(
            "
            INSERT INTO reservations (reservation_id, agent_id, workspace_id, purpose, scopes_json, status, declared_at, expires_at)
            VALUES (
                'reservation-claim-lifecycle',
                'agent-alpha',
                'workspace-main',
                'preserve only active claim scopes',
                '[{\"kind\":\"file\",\"path\":\"src/a.rs\"}]',
                'active',
                '2026-07-15T11:00:00Z',
                '2026-07-15T12:00:00Z'
            );
            INSERT INTO claims (claim_id, reservation_id, agent_id, workspace_id, repo_id, relative_path, absolute_path, purpose, action, status, expires_at, observed_exists, observed_content_hash)
            VALUES
                ('claim-lifecycle-released', 'reservation-claim-lifecycle', 'agent-alpha', 'workspace-main', 'repo-main', 'src/b.rs', '/repo/src/b.rs', 'terminal claim', 'write_file', 'released', '2026-07-15T12:00:00Z', 1, NULL),
                ('claim-lifecycle-expired', 'reservation-claim-lifecycle', 'agent-alpha', 'workspace-main', 'repo-main', 'src/b.rs', '/repo/src/b.rs', 'terminal claim', 'write_file', 'active', '2026-07-15T11:15:00Z', 1, NULL),
                ('claim-lifecycle-active', 'reservation-claim-lifecycle', 'agent-alpha', 'workspace-main', 'repo-main', 'src/c.rs', '/repo/src/c.rs', 'active claim', 'write_file', 'active', '2026-07-15T12:00:00Z', 1, NULL),
                ('claim-lifecycle-active-no-deadline', 'reservation-claim-lifecycle', 'agent-alpha', 'workspace-main', 'repo-main', 'src/d.rs', '/repo/src/d.rs', 'active claim', 'write_file', 'active', NULL, 1, NULL);
            ",
        )
        .expect("legacy rows insert");
    drop(connection);

    let mut store = Store::open_with_clock(&path, FixedClock::new(datetime!(2026-07-15 11:30 UTC)))
        .expect("legacy database migrates");

    let reservation = store
        .reservation("workspace-main", "reservation-claim-lifecycle")
        .expect("reservation reads")
        .expect("reservation exists");
    assert_eq!(
        reservation.scopes,
        vec![
            ReservationScope::file("src/a.rs"),
            ReservationScope::file("src/c.rs"),
            ReservationScope::file("src/d.rs"),
        ],
    );

    store
        .rebuild_projections()
        .expect("migration replay succeeds");
    assert_eq!(
        store
            .reservation("workspace-main", "reservation-claim-lifecycle")
            .expect("reservation reads")
            .expect("reservation exists")
            .scopes,
        vec![
            ReservationScope::file("src/a.rs"),
            ReservationScope::file("src/c.rs"),
            ReservationScope::file("src/d.rs"),
        ],
    );
}

#[test]
fn migration_promotes_slashless_directory_wait_as_directory_after_blocker_release() {
    let temp = TempDir::new().expect("temporary directory exists");
    let path = temp.path().join("legacy.sqlite");
    let connection = Connection::open(&path).expect("legacy database opens");
    connection.execute_batch(FIXTURE).expect("fixture applies");
    connection
        .execute_batch(
            "
            UPDATE reservations SET status = 'released' WHERE reservation_id = 'reservation-active';
            UPDATE claims SET status = 'released' WHERE claim_id = 'claim-active';
            UPDATE wait_queue SET status = 'canceled';
            ",
        )
        .expect("unrelated legacy blocker releases");
    connection
        .execute_batch(
            "
            INSERT INTO reservations (reservation_id, agent_id, workspace_id, purpose, scopes_json, status, declared_at, expires_at)
            VALUES (
                'reservation-directory-blocker',
                'agent-alpha',
                'workspace-main',
                'block source directory',
                '[{\"kind\":\"directory\",\"path\":\"src\"}]',
                'active',
                '2026-07-15T11:00:00Z',
                '2026-07-15T12:00:00Z'
            );
            INSERT INTO wait_queue (wait_id, request_id, agent_id, workspace_id, repo_id, worktree_id, root, branch, relative_path, action, status, requested_at, reservation_expires_at, blocking_agent_id, purpose)
            VALUES (
                'wait-slashless-directory',
                'request-slashless-directory',
                'agent-beta',
                'workspace-main',
                'repo-main',
                'worktree-main',
                '/repo',
                'main',
                'src',
                'write_directory',
                'waiting',
                '2026-07-15T11:05:00Z',
                NULL,
                'agent-alpha',
                'need source directory'
            );
            ",
        )
        .expect("legacy rows insert");
    drop(connection);

    let store = Store::open_with_clock(&path, FixedClock::new(MIGRATION_TIME))
        .expect("legacy database migrates");
    let queued = store
        .wait("workspace-main", "wait-slashless-directory")
        .expect("wait reads")
        .expect("migrated wait exists");
    assert_eq!(queued.wait_id, "wait-slashless-directory");
    assert_eq!(queued.status, "queued");

    let released = store
        .release_reservation(&request(
            "agent-alpha",
            ReservationRelease {
                reservation_id: "reservation-directory-blocker".into(),
            },
        ))
        .expect("blocker releases")
        .response;
    assert_eq!(released.status, "released");

    let promoted = store
        .wait("workspace-main", "wait-slashless-directory")
        .expect("wait reads")
        .expect("promoted wait exists");
    assert_eq!(promoted.wait_id, "wait-slashless-directory");
    assert_eq!(promoted.status, "claimable");
    assert_eq!(queued.relative_path, "src/");
    assert_eq!(promoted.relative_path, "src/");
    assert_eq!(
        promoted.reservation_id.as_deref(),
        Some("wait-slashless-directory")
    );
    assert_eq!(
        store
            .reservation("workspace-main", "wait-slashless-directory")
            .expect("reservation reads")
            .expect("promoted reservation exists")
            .scopes,
        vec![ReservationScope::directory("src")],
    );
}
