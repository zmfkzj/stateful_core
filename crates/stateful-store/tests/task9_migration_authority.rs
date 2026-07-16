use rusqlite::Connection;
use stateful_core::ReservationScope;
use stateful_store::{FixedClock, Store};
use tempfile::TempDir;
use time::macros::datetime;

const FIXTURE: &str = include_str!("fixtures/v1_persistent_state.sql");

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

    let mut store = Store::open_with_clock(
        &path,
        FixedClock::new(datetime!(2026-07-15 11:30 UTC)),
    )
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

    store.rebuild_projections().expect("migration replay succeeds");
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

    let mut store = Store::open_with_clock(
        &path,
        FixedClock::new(datetime!(2026-07-15 11:30 UTC)),
    )
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

    store.rebuild_projections().expect("migration replay succeeds");
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
