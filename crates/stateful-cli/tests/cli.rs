use clap::Parser;
use serde_json::json;
use stateful_cli::{
    Cli, CodexSandboxMode, Command, GlobalPaths, HookCommand, HookRuntime, InstallAgent,
    NotificationsCommand, OmpInstallOptions, ReconcileCommand, ReposCommand, ResumeCommand,
    SandboxCommand, SandboxFsProfile, SandboxNetworkPolicy, SandboxProcessCommand, ServerCommand,
    ToolsCommand, WatchCommand, allow_tool_for_repo, apply_omp_install, doctor_report_with_global,
    enable_repo, record_unclassified_tool_for_repo,
};
use stateful_core::{
    ActorType, AgentIdentity, AuthorizationEvent, EventData, EventPayload, NewEvent,
    RequestEnvelope, SourceKind, SourceRef, WorkspaceIdentity,
};
use stateful_store::{CommandPlan, Store};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use time::OffsetDateTime;
use uuid::Uuid;
fn journal_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path.file_name().expect("journal path should have a filename");
    path.with_file_name(format!("{}{suffix}", file_name.to_string_lossy()))
}

fn journal_wal_path(path: &Path) -> PathBuf {
    journal_sidecar_path(path, "-wal")
}

fn journal_baseline_path(path: &Path) -> PathBuf {
    journal_sidecar_path(path, ".doctor-baseline.json")
}

fn journal_file_len(path: &Path) -> u64 {
    fs::metadata(path).map(|metadata| metadata.len()).unwrap_or(0)
}

fn journal_footprint(path: &Path) -> u64 {
    journal_file_len(path).saturating_add(journal_file_len(&journal_wal_path(path)))
}

fn append_journal_event(store: &Store) {
    let request_id = Uuid::new_v4();
    let now = OffsetDateTime::now_utc();
    let request = RequestEnvelope::new(
        request_id,
        now,
        AgentIdentity {
            agent_id: "doctor-test-agent".into(),
            turn_id: Some("doctor-test-turn".into()),
            actor_id: "doctor-test-actor".into(),
            actor_type: ActorType::Agent,
            owner_id: None,
            parent_agent_id: None,
            parent_actor_id: None,
        },
        WorkspaceIdentity {
            root: "/doctor-test-repo".into(),
            workspace_id: "doctor-test-workspace".into(),
            repo_id: "doctor-test-repo".into(),
            worktree_id: "doctor-test-worktree".into(),
            branch: "main".into(),
        },
        SourceRef {
            kind: SourceKind::Cli,
            event: "doctor-test".into(),
            tool_name: None,
            source_ref: "doctor-diagnostic-fixture".into(),
        },
        json!({"intent": "doctor-diagnostic-fixture"}),
    )
    .expect("test request should be valid");
    let event = NewEvent::new(
        request_id,
        0,
        now,
        EventPayload::Authorization(AuthorizationEvent::Allowed(EventData::new(
            "doctor-diagnostic-fixture",
        ))),
    )
    .expect("test event should be valid");

    store
        .execute_command(&request, "doctor.test", |_| {
            Ok(CommandPlan {
                events: vec![event],
                response: json!({"request_id": request_id}),
                http_status: 201,
            })
        })
        .expect("persistent command should append to the journal");
}

fn write_journal_baseline(path: &Path, observed_at_unix_seconds: u64, footprint_bytes: u64) {
    fs::write(
        path,
        serde_json::to_vec(&json!({
            "observed_at_unix_seconds": observed_at_unix_seconds,
            "footprint_bytes": footprint_bytes,
        }))
        .expect("baseline fixture should serialize"),
    )
    .expect("baseline fixture should write");
}

fn assert_journal_baseline(path: &Path, footprint_bytes: u64) {
    let baseline: serde_json::Value =
        serde_json::from_slice(&fs::read(path).expect("baseline should exist"))
            .expect("baseline should be valid JSON");
    assert_eq!(
        baseline
            .get("footprint_bytes")
            .and_then(serde_json::Value::as_u64),
        Some(footprint_bytes)
    );
}

#[test]
fn parses_sandbox_run_read_only_defaults() {
    let cli = Cli::try_parse_from(["stateful", "sandbox", "run", "--command", "rg auth src"])
        .expect("sandbox run should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            purpose,
            reservation_id,
            agent_id,
            workspace_id,
            write_targets,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            json,
            command,
            sequences,
            sequence_shell,
            timeout_seconds,
            stream_events,
        }) => {
            assert_eq!(fs, SandboxFsProfile::ReadOnly);
            assert_eq!(network, SandboxNetworkPolicy::Disabled);
            assert_eq!(purpose, None);
            assert_eq!(agent_id, None);
            assert_eq!(workspace_id, None);
            assert_eq!(reservation_id, None);
            assert!(write_targets.is_empty());
            assert!(create_targets.is_empty());
            assert!(write_dirs.is_empty());
            assert!(connect_sockets.is_empty());
            assert!(!allow_signal);
            assert!(!json);
            assert!(!stream_events);
            assert_eq!(command, Some("rg auth src".to_string()));
            assert!(sequences.is_empty());
            assert_eq!(sequence_shell, None);
            assert_eq!(timeout_seconds, None);
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn parses_sandbox_run_sequence() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--fs",
        "external",
        "--purpose",
        "launch benchmark",
        "--sequence-shell",
        "/bin/zsh",
        "--sequence",
        "set -euo pipefail",
        "--sequence",
        "printf ok",
    ])
    .expect("sandbox run sequence should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            command,
            sequences,
            sequence_shell,
            ..
        }) => {
            assert_eq!(command, None);
            assert_eq!(sequence_shell, Some("/bin/zsh".to_string()));
            assert_eq!(
                sequences,
                vec!["set -euo pipefail".to_string(), "printf ok".to_string()]
            );
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn parses_sandbox_run_json_flag() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--json",
        "--command",
        "printf ok",
    ])
    .expect("sandbox run --json should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run { json, command, .. }) => {
            assert!(json);
            assert_eq!(command, Some("printf ok".to_string()));
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn parses_sandbox_process_find_json_flag() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "process",
        "find",
        "--json",
        "--contains",
        "denovo_codex_agent",
    ])
    .expect("sandbox process find --json should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Process {
            command: SandboxProcessCommand::Find { contains, .. },
        }) => {
            assert_eq!(contains, vec!["denovo_codex_agent"]);
        }
        other => panic!("expected sandbox process find command, got {other:?}"),
    }
}

#[test]
fn doctor_labels_legacy_hooks_json_without_counting_it_as_installed() {
    let temp_dir = tempfile::tempdir().expect("temp dir should create");
    let temp = temp_dir.path();
    let repo = temp.join("repo");
    let hooks_dir = repo.join(".codex");
    fs::create_dir_all(&hooks_dir).expect("hooks dir should create");
    fs::create_dir_all(repo.join(".git")).expect("fixture git dir should create");
    fs::create_dir_all(repo.join(".stateful")).expect("stateful dir should create");
    fs::write(hooks_dir.join("hooks.json"), "{}").expect("legacy hooks should write");
    fs::create_dir_all(repo.join(".stateful_core")).expect("legacy state dir should create");
    fs::write(repo.join(".stateful_core/state.db"), "legacy")
        .expect("legacy repo state db should write");
    fs::write(
        repo.join(".stateful/config.yml"),
        "protocol_version: stateful.v1\n",
    )
    .expect("repo config should write");

    let paths = GlobalPaths::new(temp.join("home"));
    let report = doctor_report_with_global(&repo, &paths);

    assert!(report.legacy_hooks_json);
    assert!(report.legacy_repo_state_db);
    assert!(!report.installed);
}

#[test]
fn doctor_reports_journal_size_rows_types_time_range_growth_and_threshold() {
    let temp_dir = tempfile::tempdir().expect("temp dir should create");
    let repo = temp_dir.path().join("repo");
    fs::create_dir_all(repo.join(".git")).expect("git marker should write");
    let paths = GlobalPaths::new(temp_dir.path().join("home"));
    Store::open(&paths.state_db).expect("journal should initialize");

    let report = serde_json::to_value(doctor_report_with_global(&repo, &paths))
        .expect("doctor report should serialize");
    let journal = report
        .get("journal")
        .expect("doctor should include sanitized journal diagnostics");
    for field in [
        "size_bytes",
        "rows",
        "event_types",
        "time_range",
        "growth_bytes",
        "growth_status",
        "threshold_bytes",
    ] {
        assert!(journal.get(field).is_some(), "journal diagnostic missing {field}");
    }
    assert_eq!(
        journal
            .get("growth_status")
            .and_then(serde_json::Value::as_str),
        Some("baseline_captured")
    );
    assert_eq!(
        journal
            .get("growth_bytes")
            .and_then(serde_json::Value::as_u64),
        Some(0)
    );
    assert!(report.get("runtime").is_some());
    assert!(report.get("capabilities").is_some());
}
#[test]
fn doctor_counts_main_and_wal_footprint_without_mutating_them() {
    let temp_dir = tempfile::tempdir().expect("temp dir should create");
    let repo = temp_dir.path().join("repo");
    fs::create_dir_all(repo.join(".git")).expect("git marker should write");
    let paths = GlobalPaths::new(temp_dir.path().join("home"));
    let store = Store::open(&paths.state_db).expect("journal should initialize");
    append_journal_event(&store);
    let wal_path = journal_wal_path(&paths.state_db);
    let main_before = fs::read(&paths.state_db).expect("main database should be readable");
    let wal_before = fs::read(&wal_path).expect("live WAL should be readable");
    assert!(!wal_before.is_empty(), "persistent store mutation should retain a live WAL");
    let footprint_before = journal_footprint(&paths.state_db);

    let report = serde_json::to_value(doctor_report_with_global(&repo, &paths))
        .expect("doctor report should serialize");

    assert_eq!(
        report
            .pointer("/journal/size_bytes")
            .and_then(serde_json::Value::as_u64),
        Some(footprint_before)
    );
    assert_eq!(
        fs::read(&paths.state_db).expect("main database should remain readable"),
        main_before,
        "doctor must not alter main database bytes"
    );
    assert_eq!(
        fs::read(&wal_path).expect("live WAL should remain readable"),
        wal_before,
        "doctor must not alter WAL bytes"
    );
}

#[test]
fn doctor_captures_then_measures_recent_physical_growth() {
    let temp_dir = tempfile::tempdir().expect("temp dir should create");
    let repo = temp_dir.path().join("repo");
    fs::create_dir_all(repo.join(".git")).expect("git marker should write");
    let paths = GlobalPaths::new(temp_dir.path().join("home"));
    let store = Store::open(&paths.state_db).expect("journal should initialize");
    let baseline_path = journal_baseline_path(&paths.state_db);
    let before = journal_footprint(&paths.state_db);

    let first = serde_json::to_value(doctor_report_with_global(&repo, &paths))
        .expect("first doctor report should serialize");
    assert_eq!(
        first
            .pointer("/journal/growth_status")
            .and_then(serde_json::Value::as_str),
        Some("baseline_captured")
    );
    assert_eq!(
        first
            .pointer("/journal/growth_bytes")
            .and_then(serde_json::Value::as_u64),
        Some(0)
    );
    assert_journal_baseline(&baseline_path, before);

    append_journal_event(&store);
    let after_mutation = journal_footprint(&paths.state_db);
    assert!(
        after_mutation > before,
        "real persistent-store mutation should grow the physical footprint"
    );
    let main_after_mutation =
        fs::read(&paths.state_db).expect("main database should be readable after mutation");
    let wal_path = journal_wal_path(&paths.state_db);
    let wal_after_mutation = fs::read(&wal_path).expect("live WAL should be readable after mutation");

    let second = serde_json::to_value(doctor_report_with_global(&repo, &paths))
        .expect("second doctor report should serialize");

    assert_eq!(
        second
            .pointer("/journal/growth_status")
            .and_then(serde_json::Value::as_str),
        Some("measured")
    );
    assert_eq!(
        second
            .pointer("/journal/growth_bytes")
            .and_then(serde_json::Value::as_u64),
        Some(after_mutation.saturating_sub(before))
    );
    assert_eq!(
        fs::read(&paths.state_db).expect("main database should remain readable"),
        main_after_mutation,
        "doctor must not alter main database bytes"
    );
    assert_eq!(
        fs::read(&wal_path).expect("live WAL should remain readable"),
        wal_after_mutation,
        "doctor must not alter WAL bytes"
    );
    assert!(
        !serde_json::to_string(&second)
            .expect("doctor report should serialize to text")
            .contains("doctor-diagnostic-fixture"),
        "doctor diagnostics must remain sanitized"
    );
}

#[test]
fn doctor_recaptures_corrupt_future_and_expired_baselines() {
    let temp_dir = tempfile::tempdir().expect("temp dir should create");
    let repo = temp_dir.path().join("repo");
    fs::create_dir_all(repo.join(".git")).expect("git marker should write");
    let paths = GlobalPaths::new(temp_dir.path().join("home"));
    let store = Store::open(&paths.state_db).expect("journal should initialize");
    append_journal_event(&store);
    let baseline_path = journal_baseline_path(&paths.state_db);
    let footprint = journal_footprint(&paths.state_db);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_secs();

    fs::write(&baseline_path, "{not-json").expect("corrupt baseline fixture should write");
    let corrupt = serde_json::to_value(doctor_report_with_global(&repo, &paths))
        .expect("doctor should tolerate a corrupt baseline");
    assert_eq!(
        corrupt
            .pointer("/journal/growth_status")
            .and_then(serde_json::Value::as_str),
        Some("baseline_captured")
    );
    assert_journal_baseline(&baseline_path, footprint);

    write_journal_baseline(&baseline_path, now.saturating_add(Duration::from_secs(3600).as_secs()), 1);
    let future = serde_json::to_value(doctor_report_with_global(&repo, &paths))
        .expect("doctor should tolerate a future baseline");
    assert_eq!(
        future
            .pointer("/journal/growth_status")
            .and_then(serde_json::Value::as_str),
        Some("baseline_captured")
    );
    assert_journal_baseline(&baseline_path, footprint);

    write_journal_baseline(
        &baseline_path,
        now.saturating_sub(Duration::from_secs(24 * 60 * 60 + 1).as_secs()),
        1,
    );
    let expired = serde_json::to_value(doctor_report_with_global(&repo, &paths))
        .expect("doctor should tolerate an expired baseline");
    assert_eq!(
        expired
            .pointer("/journal/growth_status")
            .and_then(serde_json::Value::as_str),
        Some("baseline_captured")
    );
    assert_journal_baseline(&baseline_path, footprint);
}
#[test]
fn doctor_does_not_claim_a_baseline_when_atomic_replacement_fails() {
    let temp_dir = tempfile::tempdir().expect("temp dir should create");
    let repo = temp_dir.path().join("repo");
    fs::create_dir_all(repo.join(".git")).expect("git marker should write");
    let paths = GlobalPaths::new(temp_dir.path().join("home"));
    Store::open(&paths.state_db).expect("journal should initialize");
    fs::create_dir(journal_baseline_path(&paths.state_db))
        .expect("baseline path should be blocked by a directory");

    let report = serde_json::to_value(doctor_report_with_global(&repo, &paths))
        .expect("doctor should tolerate baseline replacement failure");

    assert_eq!(
        report
            .pointer("/journal/growth_status")
            .and_then(serde_json::Value::as_str),
        Some("baseline_write_failed")
    );
}

#[test]
fn doctor_warns_at_default_five_hundred_twelve_mib_without_pruning() {
    let temp_dir = tempfile::tempdir().expect("temp dir should create");
    let repo = temp_dir.path().join("repo");
    fs::create_dir_all(repo.join(".git")).expect("git marker should write");
    let paths = GlobalPaths::new(temp_dir.path().join("home"));
    Store::open(&paths.state_db).expect("journal should initialize");
    let wal_path = journal_wal_path(&paths.state_db);
    let wal_before = fs::read(&wal_path).unwrap_or_default();
    let before = journal_file_len(&paths.state_db);
    let target_main_len = (512_u64 * 1024 * 1024).saturating_sub(wal_before.len() as u64);
    assert!(before < target_main_len, "fixture must grow the main database");
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&paths.state_db)
        .expect("journal should open");
    file.set_len(target_main_len)
        .expect("journal should extend to threshold");
    let main_before = fs::read(&paths.state_db).expect("main database should be readable");
    assert_eq!(
        journal_footprint(&paths.state_db),
        512 * 1024 * 1024,
        "threshold fixture should use the exact current footprint"
    );

    let report = serde_json::to_value(doctor_report_with_global(&repo, &paths))
        .expect("doctor report should serialize");
    assert_eq!(
        report
            .pointer("/journal/threshold_bytes")
            .and_then(serde_json::Value::as_u64),
        Some(512 * 1024 * 1024)
    );
    assert_eq!(
        report
            .pointer("/journal/size_bytes")
            .and_then(serde_json::Value::as_u64),
        Some(512 * 1024 * 1024)
    );
    assert_eq!(
        report
            .pointer("/journal/warning")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        fs::read(&paths.state_db).expect("main database should remain readable"),
        main_before,
        "doctor must not prune or vacuum the journal"
    );
    assert_eq!(
        fs::read(&wal_path).unwrap_or_default(),
        wal_before,
        "doctor must not alter the WAL"
    );
}

#[test]
fn parses_sandbox_run_write_targets_network_enabled() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--fs",
        "write-targets",
        "--network",
        "enabled",
        "--write-target",
        "README.md",
        "--create-target",
        "docs/new.md",
        "--write-dir",
        "tmp",
        "--timeout-seconds",
        "12",
        "--command",
        "printf x > README.md",
    ])
    .expect("sandbox run should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            purpose,
            reservation_id,
            agent_id,
            workspace_id,
            write_targets,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            json,
            command,
            sequences,
            sequence_shell,
            timeout_seconds,
            stream_events,
        }) => {
            assert_eq!(fs, SandboxFsProfile::WriteTargets);
            assert_eq!(network, SandboxNetworkPolicy::Enabled);
            assert_eq!(purpose, None);
            assert_eq!(reservation_id, None);
            assert_eq!(agent_id, None);
            assert_eq!(workspace_id, None);
            assert_eq!(write_targets, vec!["README.md"]);
            assert_eq!(create_targets, vec!["docs/new.md"]);
            assert_eq!(write_dirs, vec!["tmp"]);
            assert!(connect_sockets.is_empty());
            assert!(!allow_signal);
            assert!(!json);
            assert!(!stream_events);
            assert_eq!(command, Some("printf x > README.md".to_string()));
            assert!(sequences.is_empty());
            assert_eq!(sequence_shell, None);
            assert_eq!(timeout_seconds, Some(12));
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn parses_sandbox_run_git_profile() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--fs",
        "git",
        "--network",
        "enabled",
        "--timeout-seconds",
        "30",
        "--command",
        "git fetch --all",
    ])
    .expect("sandbox git profile should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            purpose,
            reservation_id,
            agent_id,
            workspace_id,
            write_targets,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            json,
            command,
            sequences,
            sequence_shell,
            timeout_seconds,
            stream_events,
        }) => {
            assert_eq!(fs, SandboxFsProfile::Git);
            assert_eq!(network, SandboxNetworkPolicy::Enabled);
            assert_eq!(purpose, None);
            assert_eq!(reservation_id, None);
            assert_eq!(agent_id, None);
            assert_eq!(workspace_id, None);
            assert!(write_targets.is_empty());
            assert!(create_targets.is_empty());
            assert!(write_dirs.is_empty());
            assert!(connect_sockets.is_empty());
            assert!(!allow_signal);
            assert!(!json);
            assert!(!stream_events);
            assert_eq!(command, Some("git fetch --all".to_string()));
            assert!(sequences.is_empty());
            assert_eq!(sequence_shell, None);
            assert_eq!(timeout_seconds, Some(30));
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn parses_sandbox_run_github_pr_profile() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--fs",
        "github-pr",
        "--network",
        "enabled",
        "--timeout-seconds",
        "30",
        "--command",
        "gh pr status",
    ])
    .expect("sandbox github-pr profile should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            purpose,
            reservation_id,
            agent_id,
            workspace_id,
            write_targets,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            json,
            command,
            sequences,
            sequence_shell,
            timeout_seconds,
            stream_events,
        }) => {
            assert_eq!(fs, SandboxFsProfile::GithubPr);
            assert_eq!(network, SandboxNetworkPolicy::Enabled);
            assert_eq!(purpose, None);
            assert_eq!(reservation_id, None);
            assert_eq!(agent_id, None);
            assert_eq!(workspace_id, None);
            assert!(write_targets.is_empty());
            assert!(create_targets.is_empty());
            assert!(write_dirs.is_empty());
            assert!(connect_sockets.is_empty());
            assert!(!allow_signal);
            assert!(!json);
            assert!(!stream_events);
            assert_eq!(command, Some("gh pr status".to_string()));
            assert!(sequences.is_empty());
            assert_eq!(sequence_shell, None);
            assert_eq!(timeout_seconds, Some(30));
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn parses_sandbox_run_build_profile() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--fs",
        "build",
        "--network",
        "enabled",
        "--timeout-seconds",
        "60",
        "--command",
        "npm test",
    ])
    .expect("sandbox build profile should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            purpose,
            reservation_id,
            agent_id,
            workspace_id,
            write_targets,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            json,
            command,
            sequences,
            sequence_shell,
            timeout_seconds,
            stream_events,
        }) => {
            assert_eq!(fs, SandboxFsProfile::Build);
            assert_eq!(network, SandboxNetworkPolicy::Enabled);
            assert_eq!(purpose, None);
            assert_eq!(reservation_id, None);
            assert_eq!(agent_id, None);
            assert_eq!(workspace_id, None);
            assert!(write_targets.is_empty());
            assert!(create_targets.is_empty());
            assert!(write_dirs.is_empty());
            assert!(connect_sockets.is_empty());
            assert!(!allow_signal);
            assert!(!json);
            assert!(!stream_events);
            assert_eq!(command, Some("npm test".to_string()));
            assert!(sequences.is_empty());
            assert_eq!(sequence_shell, None);
            assert_eq!(timeout_seconds, Some(60));
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn parses_sandbox_run_without_command_for_runtime_validation() {
    let cli = Cli::try_parse_from(["stateful", "sandbox", "run"])
        .expect("sandbox run command resolution validates missing command");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            command,
            sequences,
            sequence_shell,
            ..
        }) => {
            assert_eq!(command, None);
            assert!(sequences.is_empty());
            assert_eq!(sequence_shell, None);
        }
        other => panic!("expected sandbox run command, got {other:?}"),
    }
}

#[test]
fn parses_nested_codex_benchmark_sandbox_command() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run-nested-codex-benchmark",
        "--purpose",
        "run nested Codex chaos benchmark",
        "--agent-id",
        "agent-a",
        "--write-dir",
        "target",
        "--codex-home-root",
        "target/nested-codex-homes/run-1",
        "--timeout-seconds",
        "120",
        "--command",
        "cargo run -p stateful-bench -- run",
    ])
    .expect("nested Codex benchmark sandbox command should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::RunNestedCodexBenchmark {
            purpose,
            agent_id,
            workspace_id,
            write_dir,
            codex_home_root,
            docker_socket,
            command,
            timeout_seconds,
        }) => {
            assert_eq!(purpose, "run nested Codex chaos benchmark");
            assert_eq!(agent_id, "agent-a");
            assert_eq!(workspace_id, None);
            assert_eq!(write_dir, "target");
            assert_eq!(codex_home_root, "target/nested-codex-homes/run-1");
            assert_eq!(docker_socket, None);
            assert_eq!(command, "cargo run -p stateful-bench -- run");
            assert_eq!(timeout_seconds, Some(120));
        }
        other => panic!("expected nested Codex benchmark sandbox command, got {other:?}"),
    }
}

#[test]
fn parses_nested_codex_benchmark_sandbox_command_with_docker_socket() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run-nested-codex-benchmark",
        "--purpose",
        "run nested Codex chaos benchmark",
        "--agent-id",
        "agent-a",
        "--write-dir",
        "target",
        "--codex-home-root",
        "target/nested-codex-homes/run-1",
        "--docker-socket",
        "/tmp/colima/default/docker.sock",
        "--command",
        "cargo run -p stateful-bench -- run",
    ]);

    assert!(
        cli.is_ok(),
        "nested Codex benchmark sandbox should accept an explicit Docker socket"
    );
}

#[test]
fn nested_codex_benchmark_sandbox_requires_purpose_home_root_and_command() {
    for args in [
        vec![
            "stateful",
            "sandbox",
            "run-nested-codex-benchmark",
            "--write-dir",
            "target",
            "--codex-home-root",
            "target/nested-codex-homes/run-1",
            "--command",
            "cargo test",
        ],
        vec![
            "stateful",
            "sandbox",
            "run-nested-codex-benchmark",
            "--purpose",
            "run nested Codex chaos benchmark",
            "--write-dir",
            "target",
            "--command",
            "cargo test",
        ],
        vec![
            "stateful",
            "sandbox",
            "run-nested-codex-benchmark",
            "--purpose",
            "run nested Codex chaos benchmark",
            "--write-dir",
            "target",
            "--codex-home-root",
            "target/nested-codex-homes/run-1",
        ],
    ] {
        let error = Cli::try_parse_from(args)
            .expect_err("nested Codex benchmark command should require explicit scope");
        let message = error.to_string();
        assert!(
            message.contains("required") || message.contains("Usage:"),
            "unexpected parse error: {message}"
        );
    }
}

#[test]
fn parses_sandbox_run_external_profile() {
    let cli = Cli::try_parse_from([
        "stateful",
        "sandbox",
        "run",
        "--fs",
        "external",
        "--purpose",
        "install rebuilt binaries",
        "--write-target",
        "/opt/stateful/bin/stateful",
        "--create-target",
        "/opt/stateful/bin/stateful-bench",
        "--write-dir",
        "/opt/stateful/bin",
        "--connect-socket",
        "/private/tmp/tmux-501/default",
        "--allow-signal",
        "--network",
        "enabled",
        "--timeout-seconds",
        "10",
        "--command",
        "install -m 755 target/release/stateful /opt/stateful/bin/stateful",
    ])
    .expect("sandbox external run should parse");

    match cli.command {
        Command::Sandbox(SandboxCommand::Run {
            fs,
            network,
            purpose,
            reservation_id,
            agent_id,
            workspace_id,
            write_targets,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            json,
            timeout_seconds,
            command,
            sequences,
            sequence_shell,
            stream_events,
        }) => {
            assert_eq!(fs, SandboxFsProfile::External);
            assert_eq!(purpose, Some("install rebuilt binaries".to_string()));
            assert_eq!(reservation_id, None);
            assert_eq!(agent_id, None);
            assert_eq!(workspace_id, None);
            assert_eq!(write_targets, vec!["/opt/stateful/bin/stateful"]);
            assert_eq!(create_targets, vec!["/opt/stateful/bin/stateful-bench"]);
            assert_eq!(write_dirs, vec!["/opt/stateful/bin"]);
            assert_eq!(connect_sockets, vec!["/private/tmp/tmux-501/default"]);
            assert!(allow_signal);
            assert!(!json);
            assert_eq!(network, SandboxNetworkPolicy::Enabled);
            assert_eq!(timeout_seconds, Some(10));
            assert!(!stream_events);
            assert_eq!(
                command,
                Some(
                    "install -m 755 target/release/stateful /opt/stateful/bin/stateful".to_string()
                )
            );
            assert!(sequences.is_empty());
            assert_eq!(sequence_shell, None);
        }
        other => panic!("expected sandbox external run command, got {other:?}"),
    }
}

#[test]
fn rejects_external_run_command() {
    for args in [
        vec![
            "stateful",
            "external-run",
            "request",
            "--purpose",
            "install rebuilt binaries",
            "--write-dir",
            "/opt/stateful/bin",
            "--command",
            "true",
        ],
        vec![
            "stateful",
            "external-run",
            "approve",
            "request-123",
            "--run",
        ],
        vec!["stateful", "external-run", "run", "request-123"],
    ] {
        let error = Cli::try_parse_from(args).expect_err("external-run command should be removed");

        assert!(
            error.to_string().contains("unrecognized subcommand"),
            "unexpected parse error: {error}"
        );
    }
}

#[test]
fn git_related_stateful_subcommands_are_removed() {
    for command in ["commit", "pull", "push"] {
        let error = Cli::try_parse_from(["stateful", command])
            .expect_err("git-related stateful subcommands should be removed");

        assert!(
            error.to_string().contains("unrecognized subcommand"),
            "unexpected parse error for {command}: {error}"
        );
    }
}

#[test]
fn parses_enable_command() {
    let cli = Cli::try_parse_from(["stateful", "enable", "--repo", "/work/repo"])
        .expect("enable command should parse");

    assert!(matches!(
        cli.command,
        Command::Enable { ref repo } if repo == &Some(PathBuf::from("/work/repo"))
    ));
}

#[test]
fn parses_disable_command() {
    let cli = Cli::try_parse_from(["stateful", "disable", "--repo", "/work/repo"])
        .expect("disable command should parse");

    assert!(matches!(
        cli.command,
        Command::Disable { ref repo } if repo == &Some(PathBuf::from("/work/repo"))
    ));
}

#[test]
fn parses_watch_run_repo() {
    let cli = Cli::try_parse_from(["stateful", "watch", "run", "--repo", "/work/repo"])
        .expect("watch run should parse");

    match cli.command {
        Command::Watch(WatchCommand::Run { repo }) => {
            assert_eq!(repo, Some(PathBuf::from("/work/repo")));
        }
        other => panic!("expected watch run command, got {other:?}"),
    }
}

#[test]
fn parses_reconcile_ack() {
    let cli = Cli::try_parse_from([
        "stateful",
        "reconcile",
        "ack",
        "--resource",
        "src/auth.ts",
        "--files-reread",
        "src/auth.ts",
        "--summary",
        "Adopted human auth edit.",
        "--decision",
        "adopt",
        "--reservation-id",
        "reservation-1",
        "--agent-id",
        "agent-1",
    ])
    .expect("reconcile ack should parse");

    match cli.command {
        Command::Reconcile(ReconcileCommand::Ack {
            resources,
            files_reread,
            summary,
            decision,
            reservation_id,
            agent_id,
            workspace_id,
            conflict_with_plan,
        }) => {
            assert_eq!(resources, vec!["src/auth.ts"]);
            assert_eq!(files_reread, vec!["src/auth.ts"]);
            assert_eq!(summary, "Adopted human auth edit.");
            assert_eq!(decision, "adopt");
            assert_eq!(reservation_id.as_deref(), Some("reservation-1"));
            assert_eq!(agent_id, "agent-1");
            assert_eq!(workspace_id, None);
            assert!(!conflict_with_plan);
        }
        other => panic!("expected reconcile ack command, got {other:?}"),
    }
}

#[test]
fn rejects_codex_wrapper_command_with_read_only_tmp_sandbox() {
    let error = Cli::try_parse_from([
        "stateful",
        "codex",
        "--codex-bin",
        "/opt/codex/bin/codex",
        "--sandbox",
        "read-only-tmp",
        "exec",
        "--json",
        "-",
    ])
    .expect_err("read-only-tmp sandbox mode should be removed");

    assert!(
        error.to_string().contains("read-only-tmp"),
        "error should name the rejected sandbox mode: {error}"
    );
}

#[test]
fn parses_codex_wrapper_command_with_passthrough_sandbox_by_default() {
    let cli = Cli::try_parse_from([
        "stateful",
        "codex",
        "--codex-bin",
        "/opt/codex/bin/codex",
        "exec",
        "--json",
        "-",
    ])
    .expect("codex wrapper command should parse");

    assert!(matches!(
        cli.command,
        Command::Codex {
            ref codex_bin,
            sandbox: CodexSandboxMode::Passthrough,
            no_stateful: false,
            ref args,
        } if codex_bin == "/opt/codex/bin/codex"
            && args == &vec!["exec".to_string(), "--json".to_string(), "-".to_string()]
    ));
}

#[test]
fn parses_codex_wrapper_no_stateful_command() {
    let cli = Cli::try_parse_from(["stateful", "codex", "--no-stateful", "exec", "-"])
        .expect("codex wrapper no-stateful command should parse");

    assert!(matches!(
        cli.command,
        Command::Codex {
            no_stateful: true,
            ref args,
            ..
        } if args == &vec!["exec".to_string(), "-".to_string()]
    ));
}

#[test]
fn parses_install_yes_command() {
    let cli = Cli::try_parse_from(["stateful", "install", "--yes"])
        .expect("install command should parse");

    assert!(matches!(
        cli.command,
        Command::Install {
            yes: true,
            ref agents,
            codex_config: None,
            binary: None,
            update: false,
        }
        if agents.is_empty()
    ));
}

#[test]
fn parses_install_agent_codex_command() {
    let cli = Cli::try_parse_from([
        "stateful",
        "install",
        "--agent",
        "codex",
        "--yes",
        "--codex-config",
        "codex-home/.codex/config.toml",
        "--binary",
        "/opt/stateful/bin/stateful",
    ])
    .expect("install --agent codex command should parse");

    assert!(matches!(
        cli.command,
        Command::Install {
            yes: true,
            ref agents,
            ref codex_config,
            ref binary,
            update: false,
        } if codex_config == &Some(PathBuf::from("codex-home/.codex/config.toml"))
            && binary.as_deref() == Some("/opt/stateful/bin/stateful")
            && agents == &vec![InstallAgent::Codex]
    ));
}

#[test]
fn parses_install_agent_omp_command() {
    let cli = Cli::try_parse_from(["stateful", "install", "--agent", "omp", "--yes", "--update"])
        .expect("install --agent omp command should parse");

    assert!(matches!(
        cli.command,
        Command::Install {
            yes: true,
            ref agents,
            update: true,
            ..
        } if agents == &vec![InstallAgent::Omp]
    ));
}

#[test]
fn omp_extension_uses_strict_agent_id_identity() {
    let temp_dir = tempfile::tempdir().expect("temp dir should create");
    let temp = temp_dir.path();
    let agent_dir = temp.join("agent");

    apply_omp_install(OmpInstallOptions {
        yes: true,
        paths: GlobalPaths::new(temp.join("home")),
        binary_path: "/usr/local/bin/stateful".to_string(),
        project_config_path: None,
        omp_agent_dir: Some(agent_dir.clone()),
        update: true,
    })
    .expect("OMP install should write generated extension");

    let extension = fs::read_to_string(agent_dir.join("extensions/stateful-omp-extension.js"))
        .expect("generated OMP extension should be readable");

    assert!(extension.contains("function agentIdFragmentFromString"));
    assert!(extension.contains("function sessionManagerString"));
    assert!(extension.contains("function detectAgentId(_event, ctx)"));
    assert!(extension.contains("sessionManagerString(ctx, \"getSessionId\", sessionIdFromString)"));
    assert!(
        extension.contains("sessionManagerString(ctx, \"getLeafId\", agentIdFragmentFromString)")
    );
    assert!(extension.contains("`omp-${sessionId}-${leafId}`"));
    assert!(extension.contains("agent_id"));
    assert!(!extension.contains("event?.agentId"));
    assert!(!extension.contains("event?.agent_id"));
    assert!(!extension.contains("event?.agent?.id"));
    assert!(!extension.contains("ctx?.agentId"));
    assert!(!extension.contains("ctx?.agent_id"));
    assert!(!extension.contains("ctx?.agent?.id"));
    assert!(!extension.contains("event?.sessionId"));
    assert!(!extension.contains("event?.session_id"));
    assert!(!extension.contains("event?.session?.id"));
    assert!(!extension.contains("ctx?.sessionId"));
    assert!(!extension.contains("ctx?.session_id"));
    assert!(!extension.contains("ctx?.session?.id"));
    assert!(!extension.contains("ctx?.runtime?.sessionId"));
    assert!(!extension.contains("ctx?.runtime?.session?.id"));
    assert!(!extension.contains("process.env.STATEFUL_SESSION_ID"));
    assert!(!extension.contains("sessionIdFromSessionManager"));
    assert!(!extension.contains("function processAgentId"));
    assert!(!extension.contains("omp-pid-"));
    assert!(extension.contains("missingAgentIdReason"));
}

#[test]
fn rejects_install_codex_subcommand() {
    assert!(Cli::try_parse_from(["stateful", "install", "codex", "--yes"]).is_err());
}

#[test]
fn parses_repos_list_command() {
    let cli = Cli::try_parse_from(["stateful", "repos", "list"])
        .expect("repos list command should parse");

    assert!(matches!(cli.command, Command::Repos(ReposCommand::List)));
}

#[test]
fn parses_tools_allow_list_and_deny_commands() {
    let cli = Cli::try_parse_from([
        "stateful",
        "tools",
        "allow",
        "mcp__codex_apps__github__merge_pull_request",
        "--repo",
        "/workspace/repo",
    ])
    .expect("tools allow command should parse");
    assert!(matches!(
        cli.command,
        Command::Tools(ToolsCommand::Allow {
            ref tool_name,
            ref repo,
        }) if tool_name == "mcp__codex_apps__github__merge_pull_request"
            && repo.as_deref() == Some(std::path::Path::new("/workspace/repo"))
    ));

    let cli = Cli::try_parse_from(["stateful", "tools", "list"])
        .expect("tools list command should parse");
    assert!(matches!(
        cli.command,
        Command::Tools(ToolsCommand::List { repo: None })
    ));

    let cli = Cli::try_parse_from(["stateful", "tools", "deny", "spawn_agent"])
        .expect("tools deny command should parse");
    assert!(matches!(
        cli.command,
        Command::Tools(ToolsCommand::Deny {
            ref tool_name,
            repo: None,
        }) if tool_name == "spawn_agent"
    ));
}

#[test]
fn tools_list_prints_allowed_and_unclassified_tools() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let root = temp.path();
    let paths = GlobalPaths::new(root.join("home"));
    let repo = root.join("repo");
    fs::create_dir_all(repo.join(".git")).expect("git directory should be creatable");
    enable_repo(&paths, &repo).expect("repo should enable");
    allow_tool_for_repo(&paths, &repo, "KnownTool").expect("tool should be allowed");
    record_unclassified_tool_for_repo(&paths, &repo, "FutureWriteTool")
        .expect("unclassified tool should record");

    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["tools", "list", "--repo"])
        .arg(&repo)
        .env_clear()
        .env("STATEFUL_HOME", &paths.home)
        .output()
        .expect("tools list should run");

    assert!(
        output.status.success(),
        "tools list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("tools list should print json");
    assert_eq!(
        json["allowed_tools"],
        serde_json::json!([
            "multi_agent_v1spawn_agent",
            "multi_agent_v1wait_agent",
            "multi_agent_v1close_agent",
            "multi_agent_v1resume_agent",
            "mcp__openaiDeveloperDocs__fetch_openai_doc",
            "mcp__openaiDeveloperDocs__search_openai_docs",
            "multi_agent_v1send_input",
            "task",
            "yield",
            "parallel_tool_calls",
            "lsp",
            "glob",
            "goal",
            "ask",
            "ast_grep",
            "browser",
            "find",
            "generate_image",
            "grep",
            "hub",
            "irc",
            "job",
            "read",
            "report_tool_issue",
            "search",
            "search_tool_bm25",
            "todo",
            "web_search",
            "KnownTool"
        ])
    );
    assert_eq!(
        json["unclassified_tools"],
        serde_json::json!(["FutureWriteTool"])
    );
}

#[test]
fn parses_notifications_poll_command() {
    let cli = Cli::try_parse_from([
        "stateful",
        "notifications",
        "poll",
        "--agent-id",
        "s1",
        "--workspace-id",
        "w1",
    ])
    .expect("notifications poll command should parse");

    assert!(matches!(
        cli.command,
        Command::Notifications(NotificationsCommand::Poll {
            ref agent_id,
            ref workspace_id,
        }) if agent_id.as_deref() == Some("s1")
            && workspace_id.as_deref() == Some("w1")
    ));
}

#[test]
fn parses_resume_next_command() {
    let cli = Cli::try_parse_from([
        "stateful",
        "resume",
        "next",
        "--agent-id",
        "s1",
        "--workspace-id",
        "w1",
    ])
    .expect("resume next command should parse");

    assert!(matches!(
        cli.command,
        Command::Resume(ResumeCommand::Next {
            ref agent_id,
            ref workspace_id,
        }) if agent_id.as_deref() == Some("s1")
            && workspace_id.as_deref() == Some("w1")
    ));
}

#[test]
fn hook_codex_pre_tool_use_command_parses() {
    let cli = Cli::try_parse_from(["stateful", "hook", "codex", "pre-tool-use"])
        .expect("hook codex pre-tool-use command should parse");

    assert!(matches!(
        cli.command,
        Command::Hook(HookRuntime::Codex {
            command: HookCommand::PreToolUse,
        })
    ));
}

#[test]
fn hook_omp_pre_tool_use_command_parses() {
    let cli = Cli::try_parse_from(["stateful", "hook", "omp", "pre-tool-use"])
        .expect("hook omp pre-tool-use command should parse");

    assert!(matches!(
        cli.command,
        Command::Hook(HookRuntime::Omp {
            command: HookCommand::PreToolUse,
        })
    ));
}

#[test]
fn hook_legacy_pre_tool_use_command_is_rejected() {
    assert!(Cli::try_parse_from(["stateful", "hook", "pre-tool-use"]).is_err());
}

#[test]
fn reservation_declare_command_parses_file_scopes() {
    let cli = Cli::try_parse_from([
        "stateful",
        "reservation",
        "declare",
        "--agent-id",
        "s1",
        "--workspace-id",
        "w1",
        "--purpose",
        "Fix auth validation behavior.",
        "src/auth.ts",
        "src/session/",
    ])
    .expect("reservation declare command should parse");

    assert!(matches!(
        cli.command,
        Command::Reservation(stateful_cli::ReservationCommand::Declare {
            ref agent_id,
            ref workspace_id,
            ref purpose,
            ref files_planned,
        }) if agent_id.as_deref() == Some("s1")
            && workspace_id.as_deref() == Some("w1")
            && purpose == "Fix auth validation behavior."
            && files_planned == &vec!["src/auth.ts".to_string(), "src/session/".to_string()]
    ));
}

#[test]
fn reservation_declare_command_can_default_agent_and_workspace() {
    let cli = Cli::try_parse_from([
        "stateful",
        "reservation",
        "declare",
        "--purpose",
        "Fix auth validation behavior.",
        "src/auth.ts",
    ])
    .expect("reservation declare command should parse without explicit identity flags");

    assert!(matches!(
        cli.command,
        Command::Reservation(stateful_cli::ReservationCommand::Declare {
            agent_id: None,
            workspace_id: None,
            ref purpose,
            ref files_planned,
        }) if purpose == "Fix auth validation behavior."
            && files_planned == &vec!["src/auth.ts".to_string()]
    ));
}

#[test]
fn reservation_declare_command_requires_at_least_one_file() {
    let error = Cli::try_parse_from([
        "stateful",
        "reservation",
        "declare",
        "--purpose",
        "Fix auth validation behavior.",
    ])
    .expect_err("reservation declare without files should fail");

    assert!(
        error.to_string().contains("files_planned") || error.to_string().contains("FILES_PLANNED"),
        "unexpected error: {error}"
    );
}

#[test]
fn reservation_claim_command_requires_granted_path() {
    let error = Cli::try_parse_from([
        "stateful",
        "reservation",
        "claim",
        "--agent-id",
        "agent-a",
        "--workspace-id",
        "w1",
        "--wait-id",
        "wait-1",
    ])
    .expect_err("reservation claim without granted path should fail");

    assert!(
        error.to_string().contains("--path"),
        "unexpected error: {error}"
    );
}

#[test]
fn reservation_claim_command_preserves_wait_id_and_granted_path() {
    let cli = Cli::try_parse_from([
        "stateful",
        "reservation",
        "claim",
        "--agent-id",
        "agent-a",
        "--workspace-id",
        "w1",
        "--reservation-id",
        "reservation-a",
        "--wait-id",
        "wait-1",
        "--path",
        "src/auth.ts",
    ])
    .expect("reservation claim command should parse");

    assert!(matches!(
        cli.command,
        Command::Reservation(stateful_cli::ReservationCommand::Claim {
            ref agent_id,
            ref workspace_id,
            ref reservation_id,
            ref wait_id,
            ref path,
        }) if agent_id.as_deref() == Some("agent-a")
            && workspace_id.as_deref() == Some("w1")
            && reservation_id.as_deref() == Some("reservation-a")
            && wait_id == "wait-1"
            && path == "src/auth.ts"
    ));
}

#[test]
fn reservation_claim_command_parses_reservation_id() {
    let cli = Cli::try_parse_from([
        "stateful",
        "reservation",
        "claim",
        "--reservation-id",
        "reservation-a",
        "--wait-id",
        "wait-1",
        "--path",
        "src/auth.ts",
    ])
    .expect("claim command should parse");

    assert!(matches!(
        cli.command,
        Command::Reservation(stateful_cli::ReservationCommand::Claim {
            ref reservation_id,
            ref wait_id,
            ref path,
            ..
        }) if reservation_id.as_deref() == Some("reservation-a")
            && wait_id == "wait-1"
            && path == "src/auth.ts"
    ));
}

#[test]
fn reservation_request_command_parses_request_id_action_and_path() {
    let cli = Cli::try_parse_from([
        "stateful",
        "reservation",
        "request",
        "--agent-id",
        "s1",
        "--workspace-id",
        "w1",
        "--request-id",
        "request-1",
        "--action",
        "write_file",
        "--path",
        "src/auth.ts",
        "--purpose",
        "Queue auth file changes.",
    ])
    .expect("reservation request command should parse");

    assert!(matches!(
        cli.command,
        Command::Reservation(stateful_cli::ReservationCommand::Request {
            ref agent_id,
            ref workspace_id,
            ref request_id,
            reservation_id: _,
            ref action,
            ref path,
            ref purpose,
        }) if agent_id.as_deref() == Some("s1")
            && workspace_id.as_deref() == Some("w1")
            && request_id == "request-1"
            && action == "write_file"
            && path == "src/auth.ts"
            && purpose == "Queue auth file changes."
    ));
}

#[test]
fn reservation_cancel_command_parses_wait_id() {
    let cli = Cli::try_parse_from([
        "stateful",
        "reservation",
        "cancel",
        "--agent-id",
        "s1",
        "--workspace-id",
        "w1",
        "--wait-id",
        "wait-1",
    ])
    .expect("reservation cancel command should parse");

    assert!(matches!(
        cli.command,
        Command::Reservation(stateful_cli::ReservationCommand::Cancel {
            ref agent_id,
            ref workspace_id,
            ref wait_id,
        }) if agent_id.as_deref() == Some("s1")
            && workspace_id.as_deref() == Some("w1")
            && wait_id == "wait-1"
    ));
}

#[test]
fn server_command_parses_runtime_options() {
    let cli = Cli::try_parse_from([
        "stateful",
        "server",
        "start",
        "--host",
        "127.0.0.1",
        "--port",
        "43873",
        "--token",
        "secret-token",
        "--workspace-id",
        "w1",
    ])
    .expect("server command should parse");

    assert!(matches!(
        cli.command,
        Command::Server {
            command: Some(ServerCommand::Start {
                ref host,
                port,
                ref token,
                ref workspace_id,
                ..
            }),
            ..
        } if host == "127.0.0.1"
            && port == 43873
            && token.as_deref() == Some("secret-token")
            && workspace_id == "w1"
    ));
}

#[test]
fn parses_server_start_subcommand() {
    let cli = Cli::try_parse_from(["stateful", "server", "start", "--foreground"])
        .expect("server start should parse");

    match cli.command {
        Command::Server {
            command: Some(ServerCommand::Start { foreground, .. }),
            ..
        } => assert!(foreground),
        other => panic!("expected server start command, got {other:?}"),
    }
}

#[test]
fn parses_server_start_coordination_mode() {
    let cli = Cli::try_parse_from([
        "stateful",
        "server",
        "start",
        "--coordination-mode",
        "awareness",
    ])
    .expect("server start should parse coordination mode");

    match cli.command {
        Command::Server {
            command:
                Some(ServerCommand::Start {
                    coordination_mode, ..
                }),
            ..
        } => assert_eq!(coordination_mode, "awareness"),
        other => panic!("expected server start command, got {other:?}"),
    }
}

#[test]
fn rejects_invalid_server_start_coordination_mode() {
    let error = Cli::try_parse_from([
        "stateful",
        "server",
        "start",
        "--coordination-mode",
        "sometimes",
    ])
    .expect_err("invalid coordination mode should fail");

    assert!(error.to_string().contains("possible values"));
}

#[test]
fn parses_server_start_subcommand_as_detached_by_default() {
    let cli =
        Cli::try_parse_from(["stateful", "server", "start"]).expect("server start should parse");

    match cli.command {
        Command::Server {
            command: Some(ServerCommand::Start { foreground, .. }),
            ..
        } => assert!(!foreground),
        other => panic!("expected server start command, got {other:?}"),
    }
}

#[test]
fn parses_legacy_server_runtime_options() {
    let cli = Cli::try_parse_from([
        "stateful",
        "server",
        "--host",
        "127.0.0.1",
        "--port",
        "43874",
        "--token",
        "secret-token",
        "--workspace-id",
        "w1",
    ])
    .expect("legacy server command should parse");

    assert!(matches!(
        cli.command,
        Command::Server {
            command: None,
            ref host,
            port,
            ref token,
            ref workspace_id,
            ..
        } if host == "127.0.0.1"
            && port == 43874
            && token.as_deref() == Some("secret-token")
            && workspace_id == "w1"
    ));
}

#[test]
fn rejects_removed_lan_subcommand() {
    assert!(Cli::try_parse_from(["stateful", "lan", "serve"]).is_err());
    assert!(
        Cli::try_parse_from([
            "stateful",
            "lan",
            "join",
            "http://192.168.0.23:43873",
            "--token",
            "secret-token",
        ])
        .is_err()
    );
}

#[test]
fn parses_server_join_without_repo_enablement() {
    let cli = Cli::try_parse_from([
        "stateful",
        "server",
        "join",
        "http://192.168.0.23:43873",
        "--token",
        "secret-token",
    ])
    .expect("server join should parse");

    match cli.command {
        Command::Server {
            command:
                Some(ServerCommand::Join {
                    base_url,
                    token,
                    workspace_id,
                    allow_plain_http,
                    enable_repo,
                    binary,
                    codex_config,
                }),
            ..
        } => {
            assert_eq!(base_url, "http://192.168.0.23:43873");
            assert_eq!(token, "secret-token");
            assert_eq!(workspace_id, "shared");
            assert!(!allow_plain_http);
            assert!(!enable_repo);
            assert_eq!(binary, None);
            assert_eq!(codex_config, None);
        }
        other => panic!("expected server join command, got {other:?}"),
    }
}

#[test]
fn parses_server_join_allow_plain_http() {
    let cli = Cli::try_parse_from([
        "stateful",
        "server",
        "join",
        "http://192.168.0.23:43873",
        "--token",
        "secret-token",
        "--allow-plain-http",
    ])
    .expect("server join should parse allow-plain-http");

    match cli.command {
        Command::Server {
            command: Some(ServerCommand::Join {
                allow_plain_http, ..
            }),
            ..
        } => assert!(allow_plain_http),
        other => panic!("expected server join command, got {other:?}"),
    }
}

#[test]
fn parses_server_join_with_repo_enablement_and_install_overrides() {
    let cli = Cli::try_parse_from([
        "stateful",
        "server",
        "join",
        "http://192.168.0.23:43873",
        "--token",
        "secret-token",
        "--workspace-id",
        "w1",
        "--enable-repo",
        "--binary",
        "/opt/stateful/bin/stateful",
        "--codex-config",
        "codex-home/.codex/config.toml",
    ])
    .expect("server join should parse");

    match cli.command {
        Command::Server {
            command:
                Some(ServerCommand::Join {
                    base_url,
                    token,
                    workspace_id,
                    allow_plain_http,
                    enable_repo,
                    binary,
                    codex_config,
                }),
            ..
        } => {
            assert_eq!(base_url, "http://192.168.0.23:43873");
            assert_eq!(token, "secret-token");
            assert_eq!(workspace_id, "w1");
            assert!(!allow_plain_http);
            assert!(enable_repo);
            assert_eq!(binary.as_deref(), Some("/opt/stateful/bin/stateful"));
            assert_eq!(
                codex_config,
                Some(std::path::PathBuf::from("codex-home/.codex/config.toml"))
            );
        }
        other => panic!("expected server join command, got {other:?}"),
    }
}

#[test]
fn parses_server_restart_subcommand() {
    let cli = Cli::try_parse_from(["stateful", "server", "restart"])
        .expect("server restart should parse");

    match cli.command {
        Command::Server {
            command: Some(ServerCommand::Restart),
            ..
        } => {}
        other => panic!("expected server restart command, got {other:?}"),
    }
}
