use crate::{
    StoreError, StoreResult, configure_connection, configure_file_connection, create_v2_schema,
};
use rusqlite::{Connection, DatabaseName, OptionalExtension};
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

const V2_MARKER: &str = "stateful.v2.lease1";
const FAULT_ENV: &str = "STATEFUL_MIGRATION_FAIL_AT";
#[cfg(test)]
const FAULT_STAGES: &[&str] = &[
    "backup_created",
    "old_checkpointed",
    "old_quarantined",
    "new_validated",
    "new_promoted",
    "parent_synced",
];

pub(crate) fn open_file(path: &Path) -> StoreResult<Connection> {
    prepare_database_path(path)?;

    if manifest_exists(path)? {
        let _lock = MigrationLock::acquire(path)?;
        return open_locked(path);
    }

    if !regular_file_exists(path)? {
        ensure_no_orphan_sidecars(path)?;
        return create_new_v2(path);
    }

    let conn = Connection::open(path)?;
    configure_connection(&conn)?;
    if is_v2_database(&conn)? {
        return finish_v2_file(path, conn);
    }
    close_connection(conn)?;

    let _lock = MigrationLock::acquire(path)?;
    open_locked(path)
}

pub(crate) fn open_memory() -> StoreResult<Connection> {
    let conn = Connection::open_in_memory()?;
    configure_connection(&conn)?;
    initialize_v2(&conn)?;
    Ok(conn)
}

fn open_locked(path: &Path) -> StoreResult<Connection> {
    if manifest_exists(path)? {
        recover_interrupted_migration(path)?;
    }

    if !regular_file_exists(path)? {
        ensure_no_orphan_sidecars(path)?;
        return create_new_v2(path);
    }

    let conn = Connection::open(path)?;
    configure_connection(&conn)?;
    if is_v2_database(&conn)? {
        return finish_v2_file(path, conn);
    }
    if has_v2_marker(&conn)? {
        return Err(StoreError::InvalidState(
            "stale or mixed stateful.v2 schema; remove the database after preserving its backup"
                .to_string(),
        ));
    }
    if is_empty_database(&conn)? {
        return initialize_v2_file(path, conn);
    }
    if has_legacy_layout(&conn)? {
        return migrate_legacy(path, conn);
    }
    Err(StoreError::InvalidState(
        "unknown nonempty database schema; refusing migration".to_string(),
    ))
}

fn create_new_v2(path: &Path) -> StoreResult<Connection> {
    create_private_file(path)?;
    initialize_v2_file(path, Connection::open(path)?)
}

fn initialize_v2_file(path: &Path, conn: Connection) -> StoreResult<Connection> {
    configure_file_connection(&conn)?;
    initialize_v2(&conn)?;
    restrict_database_permissions(path)?;
    Ok(conn)
}

fn finish_v2_file(path: &Path, conn: Connection) -> StoreResult<Connection> {
    configure_file_connection(&conn)?;
    create_v2_schema(&conn)?;
    if !has_exact_v2_schema(&conn)? {
        return Err(StoreError::InvalidState(
            "stale or mixed stateful.v2 schema; remove the database after preserving its backup"
                .to_string(),
        ));
    }
    restrict_database_permissions(path)?;
    Ok(conn)
}

fn initialize_v2(conn: &Connection) -> StoreResult<()> {
    create_v2_schema(conn)?;
    if !has_v2_marker(conn)? {
        return Err(io::Error::other("v2 schema marker was not created").into());
    }
    Ok(())
}

fn migrate_legacy(path: &Path, legacy: Connection) -> StoreResult<Connection> {
    let paths = MigrationPaths::new(path)?;
    persist_manifest(&paths, MigrationStage::BackupPending)?;

    // An exclusive locking mode plus an established read transaction pins one
    // snapshot and blocks legacy writers without making SQLite backup self-deadlock.
    legacy.execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN")?;
    legacy.query_row("SELECT COUNT(*) FROM sqlite_master", [], |_| Ok(()))?;
    create_backup(&legacy, &paths.backup)?;
    persist_manifest(&paths, MigrationStage::BackupCreated)?;
    fail_if_requested(MigrationStage::BackupCreated)?;

    legacy.execute_batch("COMMIT")?;
    checkpoint(&legacy)?;
    close_connection(legacy)?;
    persist_manifest(&paths, MigrationStage::OldCheckpointed)?;
    fail_if_requested(MigrationStage::OldCheckpointed)?;

    quarantine_old_set(&paths)?;
    persist_manifest(&paths, MigrationStage::OldQuarantined)?;
    fail_if_requested(MigrationStage::OldQuarantined)?;

    create_candidate_v2(&paths)?;
    persist_manifest(&paths, MigrationStage::NewValidated)?;
    fail_if_requested(MigrationStage::NewValidated)?;

    fs::rename(&paths.candidate, &paths.main)?;
    sync_parent(&paths.parent)?;
    persist_manifest(&paths, MigrationStage::NewPromoted)?;
    fail_if_requested(MigrationStage::NewPromoted)?;

    sync_parent(&paths.parent)?;
    persist_manifest(&paths, MigrationStage::ParentSynced)?;
    fail_if_requested(MigrationStage::ParentSynced)?;

    remove_database_set(&paths.recovery)?;
    clear_manifest(&paths)?;

    let conn = Connection::open(path)?;
    finish_v2_file(path, conn)
}

fn create_backup(source: &Connection, backup: &Path) -> StoreResult<()> {
    remove_file_if_exists(backup)?;
    create_private_file(backup)?;
    source.backup(DatabaseName::Main, backup, None)?;
    restrict_file_permissions(backup)?;
    sync_file(backup)?;
    Ok(())
}

fn checkpoint(conn: &Connection) -> StoreResult<()> {
    let busy: i64 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
    if busy != 0 {
        return Err(io::Error::other("database remained busy during WAL checkpoint").into());
    }
    Ok(())
}

fn quarantine_old_set(paths: &MigrationPaths) -> StoreResult<()> {
    move_file_if_exists(&paths.main, &paths.recovery)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        move_file_if_exists(
            &database_sidecar(&paths.main, suffix)?,
            &database_sidecar(&paths.recovery, suffix)?,
        )?;
    }
    sync_parent(&paths.parent)
}

fn create_candidate_v2(paths: &MigrationPaths) -> StoreResult<()> {
    remove_database_set(&paths.candidate)?;
    create_private_file(&paths.candidate)?;

    let candidate = Connection::open(&paths.candidate)?;
    configure_file_connection(&candidate)?;
    initialize_v2(&candidate)?;
    checkpoint(&candidate)?;
    close_connection(candidate)?;

    remove_database_sidecars(&paths.candidate)?;
    restrict_file_permissions(&paths.candidate)?;
    sync_file(&paths.candidate)
}

fn recover_interrupted_migration(path: &Path) -> StoreResult<()> {
    let paths = MigrationPaths::new(path)?;
    match read_manifest(&paths)? {
        MigrationStage::BackupPending => {
            if !regular_file_exists(&paths.main)? {
                return Err(io::Error::other(
                    "migration source disappeared before backup completed",
                )
                .into());
            }
            remove_database_set(&paths.candidate)?;
            remove_database_set(&paths.recovery)?;
            remove_database_set(&paths.restore)?;
            remove_file_if_exists(&paths.backup)?;
            clear_manifest(&paths)
        }
        MigrationStage::BackupCreated
        | MigrationStage::OldCheckpointed
        | MigrationStage::OldQuarantined
        | MigrationStage::NewValidated
        | MigrationStage::NewPromoted
        | MigrationStage::ParentSynced => restore_backup_set(&paths),
    }
}

fn restore_backup_set(paths: &MigrationPaths) -> StoreResult<()> {
    if !regular_file_exists(&paths.backup)? {
        return Err(io::Error::other(
            "migration backup is missing; refusing mixed-generation recovery",
        )
        .into());
    }

    remove_database_set(&paths.main)?;
    remove_database_set(&paths.candidate)?;
    remove_database_set(&paths.recovery)?;
    remove_database_set(&paths.restore)?;

    create_private_file(&paths.restore)?;
    let mut source = File::open(&paths.backup)?;
    let mut destination = OpenOptions::new().write(true).open(&paths.restore)?;
    io::copy(&mut source, &mut destination)?;
    destination.sync_all()?;
    drop(destination);
    restrict_file_permissions(&paths.restore)?;

    fs::rename(&paths.restore, &paths.main)?;
    sync_parent(&paths.parent)?;
    clear_manifest(paths)
}

fn persist_manifest(paths: &MigrationPaths, stage: MigrationStage) -> StoreResult<()> {
    remove_file_if_exists(&paths.manifest_tmp)?;
    create_private_file(&paths.manifest_tmp)?;
    let mut file = OpenOptions::new().write(true).open(&paths.manifest_tmp)?;
    file.write_all(stage.as_str().as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);

    if regular_file_exists(&paths.manifest)? {
        restrict_file_permissions(&paths.manifest)?;
    }
    fs::rename(&paths.manifest_tmp, &paths.manifest)?;
    restrict_file_permissions(&paths.manifest)?;
    sync_parent(&paths.parent)
}

fn clear_manifest(paths: &MigrationPaths) -> StoreResult<()> {
    remove_file_if_exists(&paths.manifest)?;
    remove_file_if_exists(&paths.manifest_tmp)?;
    sync_parent(&paths.parent)
}

fn read_manifest(paths: &MigrationPaths) -> StoreResult<MigrationStage> {
    let contents = fs::read_to_string(&paths.manifest)?;
    MigrationStage::parse(contents.trim())
        .ok_or_else(|| io::Error::other("invalid migration manifest").into())
}

fn manifest_exists(path: &Path) -> StoreResult<bool> {
    let paths = MigrationPaths::new(path)?;
    regular_file_exists(&paths.manifest)
}

fn has_v2_marker(conn: &Connection) -> StoreResult<bool> {
    let has_migrations: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations')",
        [],
        |row| row.get(0),
    )?;
    if !has_migrations || !table_has_column(conn, "schema_migrations", "version")? {
        return Ok(false);
    }

    conn.query_row(
        "SELECT 1 FROM schema_migrations WHERE version = ?1 LIMIT 1",
        [V2_MARKER],
        |_| Ok(()),
    )
    .optional()
    .map(|marker| marker.is_some())
    .map_err(Into::into)
}

fn is_v2_database(conn: &Connection) -> StoreResult<bool> {
    Ok(has_v2_marker(conn)? && (has_exact_v2_schema(conn)? || is_agent_slot_index_upgrade(conn)?))
}

fn has_exact_v2_schema(conn: &Connection) -> StoreResult<bool> {
    let reference = Connection::open_in_memory()?;
    create_v2_schema(&reference)?;
    Ok(schema_objects(conn)? == schema_objects(&reference)?)
}

fn is_agent_slot_index_upgrade(conn: &Connection) -> StoreResult<bool> {
    let reference = Connection::open_in_memory()?;
    create_v2_schema(&reference)?;
    let mut previous = schema_objects(&reference)?;
    previous.retain(|(_, name, _, _)| name != "idx_unique_active_lease_agent");
    if schema_objects(conn)? != previous {
        return Ok(false);
    }
    conn.query_row(
        "SELECT NOT EXISTS(
            SELECT 1 FROM active_leases
            GROUP BY workspace_id, agent_id
            HAVING COUNT(*) > 1
        )",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn schema_objects(conn: &Connection) -> StoreResult<Vec<(String, String, String, String)>> {
    let mut statement = conn.prepare(
        "SELECT type, name, tbl_name, sql
         FROM sqlite_master
         WHERE name NOT LIKE 'sqlite_%' AND sql IS NOT NULL
         ORDER BY type, name, tbl_name, sql",
    )?;
    statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn is_empty_database(conn: &Connection) -> StoreResult<bool> {
    conn.query_row(
        "SELECT NOT EXISTS(SELECT 1 FROM sqlite_master)",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

#[derive(Clone, Copy)]
struct LegacyColumn {
    name: &'static str,
    sql_type: &'static str,
    not_null: bool,
    primary_key: bool,
}

const LEGACY_SCHEMA: &[(&str, &[LegacyColumn])] = &[
    (
        "activities",
        &[
            legacy_column("activity_id", "TEXT", false, true),
            legacy_column("agent_id", "TEXT", true, false),
            legacy_column("workspace_id", "TEXT", true, false),
            legacy_column("phase", "TEXT", true, false),
            legacy_column("expires_at", "TEXT", false, false),
        ],
    ),
    (
        "agents",
        &[
            legacy_column("agent_id", "TEXT", false, true),
            legacy_column("workspace_id", "TEXT", true, false),
            legacy_column("updated_at", "TEXT", true, false),
        ],
    ),
    (
        "claims",
        &[
            legacy_column("claim_id", "TEXT", false, true),
            legacy_column("reservation_id", "TEXT", false, false),
            legacy_column("agent_id", "TEXT", false, false),
            legacy_column("workspace_id", "TEXT", true, false),
            legacy_column("repo_id", "TEXT", false, false),
            legacy_column("relative_path", "TEXT", false, false),
            legacy_column("absolute_path", "TEXT", false, false),
            legacy_column("purpose", "TEXT", false, false),
            legacy_column("action", "TEXT", true, false),
            legacy_column("status", "TEXT", true, false),
            legacy_column("expires_at", "TEXT", false, false),
            legacy_column("observed_exists", "INTEGER", false, false),
            legacy_column("observed_content_hash", "TEXT", false, false),
        ],
    ),
    (
        "events",
        &[
            legacy_column("event_id", "TEXT", false, true),
            legacy_column("event_type", "TEXT", true, false),
            legacy_column("agent_id", "TEXT", true, false),
            legacy_column("workspace_id", "TEXT", true, false),
            legacy_column("sequence", "INTEGER", false, false),
            legacy_column("repo_id", "TEXT", false, false),
            legacy_column("worktree_id", "TEXT", false, false),
            legacy_column("root", "TEXT", false, false),
            legacy_column("branch", "TEXT", false, false),
            legacy_column("payload_json", "TEXT", true, false),
            legacy_column("created_at", "TEXT", true, false),
        ],
    ),
    (
        "notifications",
        &[
            legacy_column("notification_id", "TEXT", false, true),
            legacy_column("sequence", "INTEGER", true, false),
            legacy_column("target_agent_id", "TEXT", true, false),
            legacy_column("workspace_id", "TEXT", true, false),
            legacy_column("kind", "TEXT", true, false),
            legacy_column("payload_json", "TEXT", true, false),
            legacy_column("status", "TEXT", true, false),
            legacy_column("created_at", "TEXT", true, false),
            legacy_column("expires_at", "TEXT", false, false),
        ],
    ),
    (
        "outbox",
        &[
            legacy_column("outbox_id", "TEXT", false, true),
            legacy_column("agent_id", "TEXT", true, false),
            legacy_column("workspace_id", "TEXT", true, false),
            legacy_column("sequence", "INTEGER", true, false),
            legacy_column("event_type", "TEXT", true, false),
            legacy_column("payload_json", "TEXT", true, false),
            legacy_column("sync_status", "TEXT", true, false),
        ],
    ),
    (
        "reservations",
        &[
            legacy_column("reservation_id", "TEXT", false, true),
            legacy_column("agent_id", "TEXT", true, false),
            legacy_column("workspace_id", "TEXT", true, false),
            legacy_column("purpose", "TEXT", true, false),
            legacy_column("scopes_json", "TEXT", true, false),
            legacy_column("status", "TEXT", true, false),
            legacy_column("declared_at", "TEXT", true, false),
            legacy_column("expires_at", "TEXT", false, false),
        ],
    ),
    (
        "schema_migrations",
        &[
            legacy_column("version", "TEXT", false, true),
            legacy_column("applied_at", "TEXT", true, false),
        ],
    ),
    (
        "wait_queue",
        &[
            legacy_column("wait_id", "TEXT", false, true),
            legacy_column("request_id", "TEXT", false, false),
            legacy_column("agent_id", "TEXT", true, false),
            legacy_column("workspace_id", "TEXT", true, false),
            legacy_column("repo_id", "TEXT", false, false),
            legacy_column("worktree_id", "TEXT", false, false),
            legacy_column("root", "TEXT", false, false),
            legacy_column("branch", "TEXT", false, false),
            legacy_column("relative_path", "TEXT", true, false),
            legacy_column("action", "TEXT", true, false),
            legacy_column("status", "TEXT", true, false),
            legacy_column("requested_at", "TEXT", true, false),
            legacy_column("reservation_expires_at", "TEXT", false, false),
            legacy_column("blocking_agent_id", "TEXT", false, false),
            legacy_column("purpose", "TEXT", true, false),
        ],
    ),
];

const LEGACY_INDEXES: &[&str] = &[
    "idx_activities_agent_workspace_expires_at",
    "idx_activities_workspace_expires_at",
    "idx_agents_workspace_agent",
    "idx_claims_repo_relative_status_expires_at",
    "idx_claims_reservation_path_status",
    "idx_claims_status_expires_at",
    "idx_claims_workspace_absolute_status_expires_at",
    "idx_claims_workspace_path_status",
    "idx_events_agent_created_at",
    "idx_events_agent_sequence",
    "idx_events_workspace_created_at",
    "idx_notifications_agent_status",
    "idx_notifications_agent_workspace_status_sequence",
    "idx_notifications_status_expires_at",
    "idx_outbox_agent_sequence_sync_status",
    "idx_reservations_agent_status_expires_at",
    "idx_reservations_status_expires_at",
    "idx_wait_queue_agent_status",
    "idx_wait_queue_request_id",
    "idx_wait_queue_status_reservation_expires_at",
    "idx_wait_queue_workspace_path_status",
];

const fn legacy_column(
    name: &'static str,
    sql_type: &'static str,
    not_null: bool,
    primary_key: bool,
) -> LegacyColumn {
    LegacyColumn {
        name,
        sql_type,
        not_null,
        primary_key,
    }
}

fn has_legacy_layout(conn: &Connection) -> StoreResult<bool> {
    let objects = user_schema_objects(conn)?;
    if !objects
        .iter()
        .map(|(kind, name)| (kind.as_str(), name.as_str()))
        .eq(LEGACY_INDEXES
            .iter()
            .map(|name| ("index", *name))
            .chain(LEGACY_SCHEMA.iter().map(|(name, _)| ("table", *name))))
    {
        return Ok(false);
    }

    for (table, columns) in LEGACY_SCHEMA {
        if !has_column_shape(conn, table, columns)? {
            return Ok(false);
        }
    }

    has_legacy_marker(conn)
}

fn user_schema_objects(conn: &Connection) -> StoreResult<Vec<(String, String)>> {
    let mut statement = conn.prepare(
        "SELECT type, name FROM sqlite_master
         WHERE name NOT LIKE 'sqlite_%' AND sql IS NOT NULL
         ORDER BY type, name",
    )?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn has_legacy_marker(conn: &Connection) -> StoreResult<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 'stateful.v1.initial')
         AND NOT EXISTS(SELECT 1 FROM schema_migrations WHERE version <> 'stateful.v1.initial')",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn has_column_shape(
    conn: &Connection,
    table: &str,
    expected: &[LegacyColumn],
) -> StoreResult<bool> {
    let mut statement = conn.prepare(
        "SELECT name, type, \"notnull\", pk
         FROM pragma_table_info(?1)
         ORDER BY cid",
    )?;
    let actual = statement
        .query_map([table], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? != 0,
                row.get::<_, i64>(3)? != 0,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(actual, expected)| {
            actual.0 == expected.name
                && actual.1.eq_ignore_ascii_case(expected.sql_type)
                && actual.2 == expected.not_null
                && actual.3 == expected.primary_key
        }))
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> StoreResult<bool> {
    let mut statement = conn.prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")?;
    statement
        .query_map([table], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()
        .map(|columns| columns.iter().any(|name| name == column))
        .map_err(Into::into)
}

fn close_connection(conn: Connection) -> StoreResult<()> {
    conn.close().map_err(|(_, error)| error)?;
    Ok(())
}

fn prepare_database_path(path: &Path) -> StoreResult<()> {
    let paths = MigrationPaths::new(path)?;
    fs::create_dir_all(&paths.parent)?;
    restrict_directory_permissions(&paths.parent)?;
    for managed in [
        &paths.main,
        &paths.backup,
        &paths.manifest,
        &paths.manifest_tmp,
        &paths.candidate,
        &paths.recovery,
        &paths.restore,
        &paths.lock,
    ] {
        if regular_file_exists(managed)? {
            restrict_file_permissions(managed)?;
        }
    }
    for database in [
        &paths.main,
        &paths.candidate,
        &paths.recovery,
        &paths.restore,
    ] {
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = database_sidecar(database, suffix)?;
            if regular_file_exists(&sidecar)? {
                restrict_file_permissions(&sidecar)?;
            }
        }
    }

    Ok(())
}

fn ensure_no_orphan_sidecars(path: &Path) -> StoreResult<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        if regular_file_exists(&database_sidecar(path, suffix)?)? {
            return Err(io::Error::other("database sidecar exists without its main file").into());
        }
    }
    Ok(())
}

fn restrict_database_permissions(path: &Path) -> StoreResult<()> {
    restrict_file_permissions(path)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = database_sidecar(path, suffix)?;
        if regular_file_exists(&sidecar)? {
            restrict_file_permissions(&sidecar)?;
        }
    }
    Ok(())
}

fn remove_database_set(path: &Path) -> StoreResult<()> {
    remove_file_if_exists(path)?;
    remove_database_sidecars(path)
}

fn remove_database_sidecars(path: &Path) -> StoreResult<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        remove_file_if_exists(&database_sidecar(path, suffix)?)?;
    }
    Ok(())
}

fn move_file_if_exists(source: &Path, destination: &Path) -> StoreResult<()> {
    if !regular_file_exists(source)? {
        return Ok(());
    }
    remove_file_if_exists(destination)?;
    fs::rename(source, destination)?;
    restrict_file_permissions(destination)
}

fn remove_file_if_exists(path: &Path) -> StoreResult<()> {
    if regular_file_exists(path)? {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn create_private_file(path: &Path) -> StoreResult<()> {
    if regular_file_exists(path)? {
        return Err(
            io::Error::new(io::ErrorKind::AlreadyExists, "private file already exists").into(),
        );
    }

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?;
    restrict_file_permissions(path)
}

fn regular_file_exists(path: &Path) -> StoreResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(io::Error::other("state database path must not be a symlink").into())
        }
        Ok(metadata) if metadata_is_hard_linked(&metadata) => {
            Err(io::Error::other("state database path must not be hard-linked").into())
        }
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(io::Error::other("state database path must be a regular file").into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn metadata_is_hard_linked(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.is_file() && metadata.nlink() > 1
}

#[cfg(not(unix))]
fn metadata_is_hard_linked(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn restrict_directory_permissions(path: &Path) -> StoreResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_directory_permissions(_path: &Path) -> StoreResult<()> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file_permissions(path: &Path) -> StoreResult<()> {
    use std::os::unix::fs::PermissionsExt;
    if !regular_file_exists(path)? {
        return Ok(());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file_permissions(path: &Path) -> StoreResult<()> {
    regular_file_exists(path).map(|_| ())
}

fn sync_file(path: &Path) -> StoreResult<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> StoreResult<()> {
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> StoreResult<()> {
    Ok(())
}

fn database_sidecar(path: &Path, suffix: &str) -> StoreResult<PathBuf> {
    derived_path(path, suffix)
}

fn derived_path(path: &Path, suffix: &str) -> StoreResult<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("database path must name a file"))?;
    let mut derived = name.to_os_string();
    derived.push(suffix);
    Ok(path.with_file_name(derived))
}

struct MigrationPaths {
    parent: PathBuf,
    main: PathBuf,
    backup: PathBuf,
    manifest: PathBuf,
    manifest_tmp: PathBuf,
    candidate: PathBuf,
    recovery: PathBuf,
    restore: PathBuf,
    lock: PathBuf,
}

impl MigrationPaths {
    fn new(path: &Path) -> StoreResult<Self> {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        Ok(Self {
            parent,
            main: path.to_path_buf(),
            backup: derived_path(path, ".v1-backup")?,
            manifest: derived_path(path, ".v2-migration")?,
            manifest_tmp: derived_path(path, ".v2-migration.tmp")?,
            candidate: derived_path(path, ".v2-new")?,
            recovery: derived_path(path, ".v1-recovery")?,
            restore: derived_path(path, ".v1-restore")?,
            lock: derived_path(path, ".v2-migration.lock")?,
        })
    }
}

struct MigrationLock {
    conn: Connection,
}

impl MigrationLock {
    fn acquire(path: &Path) -> StoreResult<Self> {
        let paths = MigrationPaths::new(path)?;
        if !regular_file_exists(&paths.lock)? {
            create_private_file(&paths.lock)?;
        }
        let conn = Connection::open(&paths.lock)?;
        configure_connection(&conn)?;
        conn.execute_batch("PRAGMA journal_mode=DELETE; BEGIN EXCLUSIVE")?;
        restrict_file_permissions(&paths.lock)?;
        Ok(Self { conn })
    }
}

impl Drop for MigrationLock {
    fn drop(&mut self) {
        let _ = self.conn.execute_batch("ROLLBACK");
    }
}

#[derive(Clone, Copy)]
enum MigrationStage {
    BackupPending,
    BackupCreated,
    OldCheckpointed,
    OldQuarantined,
    NewValidated,
    NewPromoted,
    ParentSynced,
}

impl MigrationStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::BackupPending => "backup_pending",
            Self::BackupCreated => "backup_created",
            Self::OldCheckpointed => "old_checkpointed",
            Self::OldQuarantined => "old_quarantined",
            Self::NewValidated => "new_validated",
            Self::NewPromoted => "new_promoted",
            Self::ParentSynced => "parent_synced",
        }
    }

    fn parse(stage: &str) -> Option<Self> {
        match stage {
            "backup_pending" => Some(Self::BackupPending),
            "backup_created" => Some(Self::BackupCreated),
            "old_checkpointed" => Some(Self::OldCheckpointed),
            "old_quarantined" => Some(Self::OldQuarantined),
            "new_validated" => Some(Self::NewValidated),
            "new_promoted" => Some(Self::NewPromoted),
            "parent_synced" => Some(Self::ParentSynced),
            _ => None,
        }
    }
}

fn fail_if_requested(stage: MigrationStage) -> StoreResult<()> {
    #[cfg(test)]
    if TEST_FAULT_STAGE.with(std::cell::Cell::get) == Some(stage.as_str()) {
        return Err(
            io::Error::other(format!("migration fault injected at {}", stage.as_str())).into(),
        );
    }

    if std::env::var_os(FAULT_ENV).as_deref() == Some(OsStr::new(stage.as_str())) {
        return Err(
            io::Error::other(format!("migration fault injected at {}", stage.as_str())).into(),
        );
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static TEST_FAULT_STAGE: std::cell::Cell<Option<&'static str>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_initializes_v2_schema() -> StoreResult<()> {
        let conn = open_memory()?;
        assert!(has_v2_marker(&conn)?);
        let busy_timeout: i64 = conn.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
        assert_eq!(busy_timeout, crate::SQLITE_BUSY_TIMEOUT_MS as i64);
        Ok(())
    }

    #[test]
    fn new_and_existing_file_use_private_wal_v2_database() -> StoreResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.db");

        let conn = open_file(&path)?;
        assert!(has_v2_marker(&conn)?);
        let journal_mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        assert_eq!(journal_mode, "wal");
        let busy_timeout: i64 = conn.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
        assert_eq!(busy_timeout, crate::SQLITE_BUSY_TIMEOUT_MS as i64);
        close_connection(conn)?;

        let reopened = open_file(&path)?;
        assert!(has_v2_marker(&reopened)?);
        close_connection(reopened)?;
        assert_private_file(&path)?;
        assert_private_directory(directory.path())?;
        Ok(())
    }

    #[test]
    fn empty_existing_file_initializes_v2_schema() -> StoreResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.db");
        close_connection(Connection::open(&path)?)?;

        let conn = open_file(&path)?;
        assert!(has_v2_marker(&conn)?);
        close_connection(conn)
    }

    #[test]
    fn legacy_database_is_backed_up_and_replaced_without_copying_legacy_rows() -> StoreResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.db");
        write_legacy_database(&path)?;

        let conn = open_file(&path)?;
        assert!(has_v2_marker(&conn)?);
        assert!(!table_exists(&conn, "agents")?);
        close_connection(conn)?;

        let paths = MigrationPaths::new(&path)?;
        let backup = Connection::open(&paths.backup)?;
        assert!(table_exists(&backup, "agents")?);
        let rows: i64 = backup.query_row("SELECT COUNT(*) FROM agents", [], |row| row.get(0))?;
        assert_eq!(rows, 1);
        close_connection(backup)?;
        assert_private_file(&path)?;
        assert_private_file(&paths.backup)?;
        Ok(())
    }

    #[test]
    fn every_fault_stage_recovers_from_a_complete_legacy_backup() -> StoreResult<()> {
        for &stage in FAULT_STAGES {
            let directory = tempfile::tempdir()?;
            let path = directory.path().join("state.db");
            write_legacy_database(&path)?;
            set_test_fault(Some(stage))?;
            let failed = open_file(&path);
            set_test_fault(None)?;
            assert!(failed.is_err(), "stage {stage} did not fail");
            assert!(manifest_exists(&path)?);

            write_stale_sidecars(&path)?;

            set_test_fault(Some("backup_created"))?;
            let recovered_and_retried = open_file(&path);
            set_test_fault(None)?;
            assert!(
                recovered_and_retried.is_err(),
                "stage {stage} did not restore the legacy generation"
            );
            assert!(manifest_exists(&path)?);

            let conn = open_file(&path)?;
            assert!(has_v2_marker(&conn)?);
            assert!(!table_exists(&conn, "agents")?);
            close_connection(conn)?;

            let paths = MigrationPaths::new(&path)?;
            let backup = Connection::open(&paths.backup)?;
            let rows: i64 =
                backup.query_row("SELECT COUNT(*) FROM agents", [], |row| row.get(0))?;
            assert_eq!(rows, 1, "stage {stage} did not preserve legacy backup");
            close_connection(backup)?;
            assert!(!manifest_exists(&path)?);
        }
        Ok(())
    }

    #[test]
    fn linked_database_paths_fail_closed() -> StoreResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.db");
        let conn = open_file(&path)?;
        close_connection(conn)?;

        #[cfg(unix)]
        {
            let hard_link = directory.path().join("linked.db");
            fs::hard_link(&path, &hard_link)?;
            assert!(open_file(&hard_link).is_err());
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let symlink_path = directory.path().join("symlink.db");
            symlink(&path, &symlink_path)?;
            assert!(open_file(&symlink_path).is_err());
        }

        Ok(())
    }

    #[test]
    fn stale_v2_schema_fails_closed() -> StoreResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.db");
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version TEXT PRIMARY KEY);
             INSERT INTO schema_migrations VALUES ('stateful.v2.initial');",
        )?;
        close_connection(conn)?;

        assert!(matches!(open_file(&path), Err(StoreError::InvalidState(_))));
        Ok(())
    }

    #[test]
    fn current_marker_with_missing_table_fails_closed_without_repair() -> StoreResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.db");
        let conn = Connection::open(&path)?;
        initialize_v2(&conn)?;
        conn.execute_batch("DROP TABLE coordination_sequences")?;
        close_connection(conn)?;

        assert_rejected_without_modifying(&path)
    }

    #[test]
    fn current_v2_without_agent_slot_index_upgrades() -> StoreResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.db");
        let conn = Connection::open(&path)?;
        initialize_v2(&conn)?;
        conn.execute_batch("DROP INDEX idx_unique_active_lease_agent")?;
        close_connection(conn)?;

        let upgraded = open_file(&path)?;
        assert!(has_exact_v2_schema(&upgraded)?);
        close_connection(upgraded)
    }

    #[test]
    fn current_v2_with_duplicate_agent_leases_fails_closed() -> StoreResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.db");
        let conn = Connection::open(&path)?;
        initialize_v2(&conn)?;
        conn.execute_batch(
            "DROP INDEX idx_unique_active_lease_agent;
             INSERT INTO tasks VALUES
                ('workspace', 'task-1', 'agent', 'active', NULL, 'one', '{}', NULL,
                 '2026-08-02T00:00:00Z', '2026-08-02T01:00:00Z',
                 '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z'),
                ('workspace', 'task-2', 'agent', 'active', NULL, 'two', '{}', NULL,
                 '2026-08-02T00:00:00Z', '2026-08-02T01:00:00Z',
                 '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z');
             INSERT INTO active_leases VALUES
                ('batch-1', 'workspace', 'task-1', 'agent', 'exclusive_write', 'active', 1,
                 '2026-08-02T00:00:00Z', '2026-08-02T01:00:00Z', 0),
                ('batch-2', 'workspace', 'task-2', 'agent', 'exclusive_write', 'active', 1,
                 '2026-08-02T00:00:00Z', '2026-08-02T01:00:00Z', 0);",
        )?;
        close_connection(conn)?;

        assert_rejected_without_modifying(&path)
    }
    #[test]
    fn current_marker_with_legacy_agents_table_fails_closed_without_migration() -> StoreResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.db");
        let conn = Connection::open(&path)?;
        initialize_v2(&conn)?;
        conn.execute_batch(
            "CREATE TABLE agents (
                agent_id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
        )?;
        close_connection(conn)?;

        assert_rejected_without_modifying(&path)
    }

    #[test]
    fn partial_legacy_schema_fails_closed_without_migration() -> StoreResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.db");
        write_legacy_database(&path)?;
        let conn = Connection::open(&path)?;
        conn.execute_batch("DROP TABLE claims")?;
        close_connection(conn)?;

        assert_rejected_without_modifying(&path)
    }

    #[test]
    fn legacy_schema_with_extra_table_fails_closed_without_migration() -> StoreResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.db");
        write_legacy_database(&path)?;
        let conn = Connection::open(&path)?;
        conn.execute_batch("CREATE TABLE extra_data (id TEXT PRIMARY KEY)")?;
        close_connection(conn)?;

        assert_rejected_without_modifying(&path)
    }

    #[test]
    fn legacy_schema_with_extra_objects_fails_closed_without_migration() -> StoreResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.db");
        write_legacy_database(&path)?;
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "CREATE INDEX unexpected_index ON agents(updated_at);
             CREATE VIEW unexpected_view AS SELECT agent_id FROM agents;
             CREATE TRIGGER unexpected_trigger AFTER INSERT ON agents BEGIN SELECT 1; END;",
        )?;
        close_connection(conn)?;

        assert_rejected_without_modifying(&path)
    }

    #[test]
    fn session_id_lookalike_fails_closed_without_migration() -> StoreResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.db");
        let conn = Connection::open(&path)?;
        conn.execute_batch("CREATE TABLE lookalike (session_id TEXT PRIMARY KEY)")?;
        close_connection(conn)?;

        assert_rejected_without_modifying(&path)
    }

    fn assert_rejected_without_modifying(path: &Path) -> StoreResult<()> {
        let original = fs::read(path)?;
        assert!(matches!(open_file(path), Err(StoreError::InvalidState(_))));
        assert_eq!(fs::read(path)?, original);

        let paths = MigrationPaths::new(path)?;
        assert!(!manifest_exists(path)?);
        for artifact in [
            paths.backup,
            paths.candidate,
            paths.recovery,
            paths.restore,
            paths.manifest_tmp,
        ] {
            assert!(!regular_file_exists(&artifact)?);
        }
        Ok(())
    }

    fn write_legacy_database(path: &Path) -> StoreResult<()> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE schema_migrations (
                 version TEXT PRIMARY KEY,
                 applied_at TEXT NOT NULL
             );
             INSERT INTO schema_migrations VALUES ('stateful.v1.initial', '2026-05-31T00:00:00Z');
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
             CREATE INDEX idx_events_agent_created_at ON events(agent_id, created_at);
             CREATE INDEX idx_events_agent_sequence ON events(agent_id, sequence);
             CREATE TABLE agents (
                 agent_id TEXT PRIMARY KEY,
                 workspace_id TEXT NOT NULL,
                 updated_at TEXT NOT NULL
             );
             INSERT INTO agents VALUES ('legacy', 'workspace', '2026-05-31T00:00:00Z');
             CREATE INDEX idx_agents_workspace_agent ON agents(workspace_id, agent_id);
             CREATE TABLE activities (
                 activity_id TEXT PRIMARY KEY,
                 agent_id TEXT NOT NULL,
                 workspace_id TEXT NOT NULL,
                 phase TEXT NOT NULL DEFAULT 'exploring',
                 expires_at TEXT
             );
             CREATE INDEX idx_activities_workspace_expires_at ON activities(workspace_id, expires_at);
             CREATE INDEX idx_activities_agent_workspace_expires_at ON activities(agent_id, workspace_id, expires_at);
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
             CREATE INDEX idx_reservations_agent_status_expires_at ON reservations(agent_id, status, expires_at);
             CREATE INDEX idx_reservations_status_expires_at ON reservations(status, expires_at);
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
             CREATE INDEX idx_claims_workspace_path_status ON claims(workspace_id, relative_path, status);
             CREATE INDEX idx_claims_workspace_absolute_status_expires_at ON claims(workspace_id, absolute_path, status, expires_at);
             CREATE INDEX idx_claims_repo_relative_status_expires_at ON claims(repo_id, relative_path, status, expires_at);
             CREATE INDEX idx_claims_reservation_path_status ON claims(reservation_id, workspace_id, relative_path, status);
             CREATE INDEX idx_claims_status_expires_at ON claims(status, expires_at);
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
             CREATE INDEX idx_wait_queue_workspace_path_status ON wait_queue(workspace_id, relative_path, status);
             CREATE INDEX idx_wait_queue_agent_status ON wait_queue(agent_id, status);
             CREATE INDEX idx_wait_queue_status_reservation_expires_at ON wait_queue(status, reservation_expires_at);
             CREATE UNIQUE INDEX idx_wait_queue_request_id ON wait_queue(request_id) WHERE request_id IS NOT NULL;
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
             CREATE INDEX idx_notifications_agent_status ON notifications(target_agent_id, status);
             CREATE INDEX idx_notifications_agent_workspace_status_sequence ON notifications(target_agent_id, workspace_id, status, sequence);
             CREATE INDEX idx_notifications_status_expires_at ON notifications(status, expires_at);
             CREATE TABLE outbox (
                 outbox_id TEXT PRIMARY KEY,
                 agent_id TEXT NOT NULL,
                 workspace_id TEXT NOT NULL DEFAULT '',
                 sequence INTEGER NOT NULL,
                 event_type TEXT NOT NULL DEFAULT '',
                 payload_json TEXT NOT NULL DEFAULT '{}',
                 sync_status TEXT NOT NULL
             );
             CREATE INDEX idx_outbox_agent_sequence_sync_status ON outbox(agent_id, sequence, sync_status);",
        )?;
        close_connection(conn)
    }

    fn table_exists(conn: &Connection, table: &str) -> StoreResult<bool> {
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
            [table],
            |_| Ok(()),
        )
        .optional()
        .map(|table| table.is_some())
        .map_err(Into::into)
    }

    fn write_stale_sidecars(path: &Path) -> StoreResult<()> {
        for suffix in ["-wal", "-shm"] {
            let sidecar = database_sidecar(path, suffix)?;
            remove_file_if_exists(&sidecar)?;
            create_private_file(&sidecar)?;
        }
        Ok(())
    }

    fn set_test_fault(stage: Option<&'static str>) -> StoreResult<()> {
        TEST_FAULT_STAGE.set(stage);
        Ok(())
    }

    #[cfg(unix)]
    fn assert_private_file(path: &Path) -> StoreResult<()> {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o600);
        Ok(())
    }

    #[cfg(not(unix))]
    fn assert_private_file(_path: &Path) -> StoreResult<()> {
        Ok(())
    }

    #[cfg(unix)]
    fn assert_private_directory(path: &Path) -> StoreResult<()> {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o700);
        Ok(())
    }

    #[cfg(not(unix))]
    fn assert_private_directory(_path: &Path) -> StoreResult<()> {
        Ok(())
    }
}
