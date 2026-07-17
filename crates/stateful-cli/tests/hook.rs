use std::{
    collections::VecDeque,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use stateful_cli::{
    GlobalPaths, HookOutcome, OmpHookOutcome, ServerRuntime, allow_tool_for_repo,
    deny_tool_for_repo, enable_repo, handle_omp_post_tool_use_with_runtime,
    handle_omp_pre_tool_use_with_runtime, handle_omp_session_start_with_runtime,
    handle_post_tool_use_in_repo, handle_pre_tool_use, handle_pre_tool_use_in_repo,
    sync_outbox_with_runtime, tool_list_for_repo, workspace_id_for_enabled_repo,
    write_global_runtime_file,
};

fn assert_raw_bash_denied_with_sandbox_run_guidance(outcome: HookOutcome) {
    let HookOutcome::Deny { reason } = outcome else {
        panic!("raw Bash should be denied");
    };

    assert!(
        reason.contains("stateful sandbox run"),
        "reason `{reason}` should direct callers to stateful sandbox run"
    );
}

fn assert_bash_denial_mentions(outcome: HookOutcome, expected: &str) {
    let HookOutcome::Deny { reason } = outcome else {
        panic!("expected Bash denial");
    };
    assert!(
        reason.contains(expected),
        "reason `{reason}` should contain `{expected}`"
    );
}

fn assert_bash_denial_mentions_all(outcome: HookOutcome, expected: &[&str]) {
    let HookOutcome::Deny { reason } = outcome else {
        panic!("expected Bash denial");
    };
    for expected in expected {
        assert!(
            reason.contains(expected),
            "reason `{reason}` should contain `{expected}`"
        );
    }
}

fn trusted_stateful_path() -> String {
    std::env::current_exe()
        .expect("test executable path should resolve")
        .to_string_lossy()
        .into_owned()
}

#[cfg(feature = "codex-benchmark")]
fn trusted_tmux_path() -> &'static str {
    "/opt/homebrew/bin/tmux"
}

fn assert_current_agent_context_absent(repo_root: &Path) {
    assert!(
        !repo_root
            .join(".stateful_core/runtime/session.json")
            .exists(),
        "legacy current agent context file should not exist"
    );
    assert!(
        !repo_root.join(".stateful_core/runtime/sessions").exists(),
        "agent-bound current context directory should not exist"
    );
}

#[test]
fn session_start_registers_renders_injects_and_acks_initial_context() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let identity = r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#;
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        identity,
        identity,
        r#"{"workspace_id":"w1","agent_id":"codex-agent-1"}"#,
        identity,
        r#"{"from_version":0,"workspace_version":1,"changed":true,"reset_required":false,"delivery_id":"delivery-1","sequence":1,"items":[],"prompt_text":"Initial context"}"#,
        identity,
        r#"{"acknowledged_version":1,"cursor":1}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "codex-agent-1",
        "thread_id": "codex-agent-1",
        "cwd": repo_root,
        "hook_event_name": "SessionStart"
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "session-start"],
        &input,
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let health_identity = rx.recv().expect("health identity request should arrive");
    assert!(health_identity.contains("GET /v2/runtime/identity?"));
    let register_identity = rx.recv().expect("register identity request should arrive");
    assert!(register_identity.contains("GET /v2/runtime/identity?"));
    let register = rx.recv().expect("register request should arrive");
    assert!(
        register.contains("POST /v2/session/register HTTP/1.1"),
        "{register}"
    );
    let register_body = request_json_body(&register);
    assert_eq!(register_body["protocol_version"], "stateful.v2");
    assert_eq!(register_body["agent"]["agent_id"], "codex-agent-1");

    let render_identity = rx.recv().expect("render identity request should arrive");
    assert!(render_identity.contains("GET /v2/runtime/identity?"));
    let render = rx.recv().expect("context render request should arrive");
    assert!(render.contains("POST /v2/context/render HTTP/1.1"));
    assert_eq!(request_json_body(&render)["payload"]["mode"], "brief");
    let ack_identity = rx.recv().expect("ack identity request should arrive");
    assert!(ack_identity.contains("GET /v2/runtime/identity?"));
    let ack = rx.recv().expect("context acknowledgement should arrive");
    assert!(ack.contains("POST /v2/context/ack HTTP/1.1"));
    let ack_body = request_json_body(&ack);
    assert_eq!(ack_body["payload"]["delivery_id"], "delivery-1");
    assert_eq!(ack_body["payload"]["sequence"], 1);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Initial context"));
}

#[test]
fn first_prompt_captures_goal_and_later_prompts_deliver_only_new_versions() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let identity = r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":2,"capabilities":["presence"]}"#;
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        identity,
        r#"{"presence":{"goal_excerpt":null}}"#,
        identity,
        r#"{"agent_id":"codex-agent-1","goal_excerpt":"work on auth"}"#,
        identity,
        r#"{"from_version":1,"workspace_version":2,"changed":true,"reset_required":false,"delivery_id":"delivery-2","sequence":2,"items":[],"prompt_text":"Prompt context"}"#,
        identity,
        r#"{"acknowledged_version":2,"cursor":2}"#,
        identity,
        r#"{"presence":{"goal_excerpt":"work on auth"}}"#,
        identity,
        r#"{"from_version":2,"workspace_version":2,"changed":false,"reset_required":false,"items":[],"prompt_text":""}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let input = serde_json::json!({
        "agent_id": "codex-agent-1",
        "cwd": repo_root,
        "hook_event_name": "UserPromptSubmit",
        "prompt": "work on auth"
    })
    .to_string();

    let first = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "user-prompt-submit"],
        &input,
    );
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let health = rx.recv().expect("health request should arrive");
    assert!(health.contains("GET /v2/runtime/identity?"));
    let current = rx.recv().expect("presence lookup should arrive");
    assert!(current.contains("GET /v2/current?"));
    let update_identity = rx.recv().expect("presence update identity should arrive");
    assert!(update_identity.contains("GET /v2/runtime/identity?"));
    let update = rx.recv().expect("first prompt update should arrive");
    assert!(update.contains("POST /v2/presence/update HTTP/1.1"));
    let update_body = request_json_body(&update);
    assert_eq!(update_body["payload"]["goal_excerpt"], "work on auth");
    let render_identity = rx.recv().expect("render identity should arrive");
    assert!(render_identity.contains("GET /v2/runtime/identity?"));
    let render = rx.recv().expect("render request should arrive");
    assert!(render.contains("POST /v2/context/render HTTP/1.1"));
    let ack_identity = rx.recv().expect("ack identity should arrive");
    assert!(ack_identity.contains("GET /v2/runtime/identity?"));
    let ack = rx.recv().expect("ack request should arrive");
    assert!(ack.contains("POST /v2/context/ack HTTP/1.1"));
    assert!(String::from_utf8_lossy(&first.stdout).contains("Prompt context"));

    let second = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "user-prompt-submit"],
        &input,
    );
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let health = rx.recv().expect("second health request should arrive");
    assert!(health.contains("GET /v2/runtime/identity?"));
    let current = rx.recv().expect("second presence lookup should arrive");
    assert!(current.contains("GET /v2/current?"));
    let render_identity = rx.recv().expect("second render identity should arrive");
    assert!(render_identity.contains("GET /v2/runtime/identity?"));
    let render = rx.recv().expect("second render request should arrive");
    assert!(render.contains("POST /v2/context/render HTTP/1.1"));
    assert!(second.stdout.is_empty());
}

#[test]
fn normal_read_posts_structural_completion_with_one_operation_id() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo source should create");
    fs::write(repo_root.join("src/read.txt"), "exact contents\n")
        .expect("read fixture should write");
    enable_test_repo(&paths, &repo_root);
    let identity = r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#;
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        identity,
        identity,
        r#"{"operation_id":"read-call-1","path":"src/read.txt"}"#,
        identity,
        identity,
        r#"{"operation_id":"read-call-1","path":"src/read.txt","status":"stabilized"}"#,
        identity,
        r#"{"status":"ok"}"#,
        identity,
        r#"{"status":"ok"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let input = serde_json::json!({
        "agent_id": "codex-agent-1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "Read",
        "tool_use_id": "read-call-1",
        "tool_input": { "file_path": "src/read.txt" }
    })
    .to_string();
    let pre = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );
    assert!(
        pre.status.success(),
        "{}",
        String::from_utf8_lossy(&pre.stderr)
    );
    let health = rx.recv().expect("pre-tool health request should arrive");
    assert!(health.contains("GET /v2/runtime/identity?"));
    let start_identity = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("read start identity request should arrive");
    assert!(start_identity.contains("GET /v2/runtime/identity?"));
    let start = rx.recv().expect("read start request should arrive");
    assert!(start.contains("POST /v2/read/start HTTP/1.1"), "{start}");
    let start_body = request_json_body(&start);
    assert_eq!(start_body["payload"]["operation_id"], "read-call-1");
    assert_eq!(start_body["payload"]["path"], "src/read.txt");
    assert!(start_body["payload"]["before"]["sha256"].is_string());

    let post = input.replace("\"PreToolUse\"", "\"PostToolUse\"");
    let post = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "post-tool-use"],
        &post,
    );
    assert!(
        post.status.success(),
        "{}",
        String::from_utf8_lossy(&post.stderr)
    );
    let health = rx.recv().expect("post-tool health request should arrive");
    assert!(health.contains("GET /v2/runtime/identity?"));
    let complete_identity = rx
        .recv()
        .expect("read complete identity request should arrive");
    assert!(complete_identity.contains("GET /v2/runtime/identity?"));
    let complete = rx.recv().expect("read completion should arrive");
    assert!(complete.contains("POST /v2/read/complete HTTP/1.1"));
    let complete_body = request_json_body(&complete);
    assert_eq!(complete_body["payload"]["operation_id"], "read-call-1");
    assert_eq!(
        complete_body["payload"]["classification"],
        "structural_summary"
    );
    assert!(complete_body["payload"].get("after").is_none());
    let result_identity = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("read result identity request should arrive");
    assert!(result_identity.contains("GET /v2/runtime/identity?"));
    let result = request_json_body(
        &rx.recv_timeout(Duration::from_secs(2))
            .expect("read result should arrive"),
    );
    assert_eq!(result["payload"]["kind"], "tool_result");
    assert_eq!(result["payload"]["outcome"], "structural_summary");
    let heartbeat_identity = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("heartbeat identity request should arrive");
    assert!(heartbeat_identity.contains("GET /v2/runtime/identity?"));
    let heartbeat = request_json_body(
        &rx.recv_timeout(Duration::from_secs(2))
            .expect("read heartbeat should arrive"),
    );
    assert_eq!(heartbeat["payload"]["kind"], "heartbeat");
}

#[test]
fn partial_or_truncated_read_completes_without_baseline() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo source should create");
    fs::write(repo_root.join("src/read.txt"), "partial contents\n")
        .expect("read fixture should write");
    enable_test_repo(&paths, &repo_root);
    let identity = r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#;
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        identity,
        identity,
        r#"{"operation_id":"partial-read-1","path":"src/read.txt"}"#,
        identity,
        identity,
        r#"{"status":"recorded"}"#,
        r#"{"status":"updated"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let input = serde_json::json!({
        "agent_id": "codex-agent-1",
        "cwd": repo_root,
        "tool_name": "Read",
        "tool_call_id": "partial-read-1",
        "is_complete": false,
        "tool_input": { "file_path": "src/read.txt", "offset": 1, "limit": 1 }
    })
    .to_string();
    let pre = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );
    assert!(
        pre.status.success(),
        "{}",
        String::from_utf8_lossy(&pre.stderr)
    );
    for _ in 0..2 {
        let request = rx.recv().expect("pre-read request should arrive");
        assert!(request.contains("GET /v2/runtime/identity?"));
    }
    let start = rx.recv().expect("read start should arrive");
    assert!(start.contains("POST /v2/read/start HTTP/1.1"));

    let post = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "post-tool-use"],
        &input,
    );
    assert!(
        post.status.success(),
        "{}",
        String::from_utf8_lossy(&post.stderr)
    );
    for _ in 0..2 {
        let request = rx.recv().expect("post-read identity request should arrive");
        assert!(request.contains("GET /v2/runtime/identity?"));
    }
    let complete = rx
        .recv_timeout(Duration::from_millis(200))
        .expect("partial read completion should arrive");
    assert!(
        complete.contains("POST /v2/read/complete HTTP/1.1"),
        "{complete}"
    );
    let body = request_json_body(&complete);
    assert_eq!(body["payload"]["operation_id"], "partial-read-1");
    assert_eq!(body["payload"]["classification"], "partial");
    assert!(body["payload"].get("after").is_none());
    let update = rx
        .recv()
        .expect("partial read presence update should arrive");
    assert!(
        update.contains("POST /v2/presence/update HTTP/1.1"),
        "{update}"
    );
    let body = request_json_body(&update);
    assert_eq!(body["payload"]["kind"], "tool_result");
    assert_eq!(body["payload"]["outcome"], "partial");
}

#[test]
fn failed_optional_read_presence_update_does_not_skip_prior_completion() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo source should create");
    fs::write(repo_root.join("src/read.txt"), "partial contents\n")
        .expect("read fixture should write");
    enable_test_repo(&paths, &repo_root);
    let (runtime, requests) = spawn_server_dropping_presence_update();
    write_global_runtime_file(&paths, &runtime).expect("runtime file should write");
    let input = serde_json::json!({
        "agent_id": "codex-agent-1",
        "cwd": repo_root,
        "tool_name": "Read",
        "tool_call_id": "partial-read-presence-failure",
        "is_complete": false,
        "tool_input": { "file_path": "src/read.txt", "offset": 1, "limit": 1 }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "post-tool-use"],
        &input,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = requests.try_iter().collect::<Vec<_>>();
    let complete = requests
        .iter()
        .position(|request| request.starts_with("POST /v2/read/complete "));
    let update = requests
        .iter()
        .position(|request| request.starts_with("POST /v2/presence/update "));
    assert!(
        matches!((complete, update), (Some(complete), Some(update)) if complete < update),
        "completion must precede failed optional presence update: {requests:?}"
    );
}

#[test]
fn omp_raw_reads_use_the_underlying_file_for_lifecycle_fingerprints() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo source should create");
    fs::write(repo_root.join("src/read.txt"), "contents\n").expect("read fixture should write");
    enable_test_repo(&paths, &repo_root);
    let identity = r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#;
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        identity,
        identity,
        r#"{"status":"started"}"#,
        identity,
        identity,
        r#"{"status":"updated"}"#,
        identity,
        r#"{"status":"completed"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let input = serde_json::json!({
        "agent_id": "omp-agent-1",
        "workspace_id": runtime.workspace_id,
        "cwd": repo_root,
        "operation_id": "omp-read-raw-1",
        "exact_read_candidate": true,
        "tool_name": "read",
        "tool_input": { "path": "src/read.txt:raw" }
    })
    .to_string();

    let pre = run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "pre-tool-use"], &input);
    assert!(
        pre.status.success(),
        "{}",
        String::from_utf8_lossy(&pre.stderr)
    );
    for _ in 0..2 {
        let identity = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("read-start identity should arrive");
        assert!(identity.starts_with("GET /v2/runtime/identity?"));
    }
    let start = request_json_body(
        &rx.recv_timeout(Duration::from_secs(2))
            .expect("read start should arrive"),
    );
    assert_eq!(start["payload"]["path"], "src/read.txt");

    let post = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "omp", "post-tool-use"],
        &input,
    );
    assert!(
        post.status.success(),
        "{}",
        String::from_utf8_lossy(&post.stderr)
    );
    let mut post_requests = Vec::new();
    while let Ok(request) = rx.recv_timeout(Duration::from_millis(200)) {
        post_requests.push(request);
    }
    let completion_index = post_requests
        .iter()
        .position(|request| request.starts_with("POST /v2/read/complete "))
        .expect("read completion should arrive");
    let heartbeat_index = post_requests
        .iter()
        .position(|request| request.starts_with("POST /v2/presence/update "))
        .expect("heartbeat should arrive");
    assert!(
        completion_index < heartbeat_index,
        "read completion must precede heartbeat: {post_requests:?}"
    );
    let complete = request_json_body(&post_requests[completion_index]);
    assert_eq!(complete["payload"]["path"], "src/read.txt");
    assert!(complete["payload"]["after"]["sha256"].is_string());
    let heartbeat = request_json_body(&post_requests[heartbeat_index]);
    assert_eq!(heartbeat["payload"]["kind"], "heartbeat");
}

#[test]
fn omp_namespaced_nested_raw_read_selectors_use_file_target_and_are_partial() {
    for selector in [
        "src/read.txt:2-4",
        "src/read.txt:raw:2-4",
        "src/read.txt:2-4:raw",
    ] {
        let temp = tempfile::tempdir().expect("temp dir should create");
        let paths = GlobalPaths::new(temp.path().join("home"));
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(repo_root.join("src")).expect("repo source should create");
        fs::write(repo_root.join("src/read.txt"), "contents\n").expect("read fixture should write");
        enable_test_repo(&paths, &repo_root);
        let (runtime, rx) = spawn_v2_started_deny_server();
        write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
        let input = serde_json::json!({
            "agent_id": "omp-agent-1",
            "workspace_id": runtime.workspace_id,
            "cwd": repo_root,
            "operation_id": "omp-read-nested-selector",
            "tool_name": "functions.read",
            "tool_input": { "path": selector }
        })
        .to_string();

        let pre = run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "pre-tool-use"], &input);
        assert!(
            String::from_utf8_lossy(&pre.stdout).contains(r#""decision":"allow""#),
            "nested selector should be allowed: {}",
            String::from_utf8_lossy(&pre.stdout)
        );
        let start = request_json_body(
            &rx.recv_timeout(Duration::from_secs(2))
                .expect("read start should arrive"),
        );
        assert_eq!(start["payload"]["path"], "src/read.txt");

        let post = run_hook_subprocess(
            &repo_root,
            &paths,
            &["hook", "omp", "post-tool-use"],
            &input,
        );
        assert!(
            post.status.success(),
            "{}",
            String::from_utf8_lossy(&post.stderr)
        );
        let mut requests = Vec::new();
        while let Ok(request) = rx.recv_timeout(Duration::from_millis(250)) {
            requests.push(request);
            if requests.len() == 3 {
                break;
            }
        }
        assert_eq!(
            requests.len(),
            3,
            "read completion, result, and heartbeat should arrive; stderr: {}; requests: {requests:?}",
            String::from_utf8_lossy(&post.stderr)
        );
        let complete = requests
            .iter()
            .find(|request| request.starts_with("POST /v2/read/complete "))
            .map(|request| request_json_body(request))
            .expect("read completion should arrive");
        assert_eq!(complete["payload"]["path"], "src/read.txt");
        assert_eq!(complete["payload"]["classification"], "partial");
        assert!(complete["payload"].get("after").is_none());
        assert!(requests.iter().any(|request| {
            let body = request_json_body(request);
            body["payload"]["kind"] == "tool_result" && body["payload"]["outcome"] == "partial"
        }));
        assert!(
            requests
                .iter()
                .any(|request| { request_json_body(request)["payload"]["kind"] == "heartbeat" })
        );
    }
}

#[test]
fn denied_recognized_test_commands_do_not_emit_testing_starts() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_v2_started_deny_server();
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let codex = serde_json::json!({
        "agent_id": "codex-agent-1",
        "cwd": repo_root,
        "tool_name": "Bash",
        "tool_input": { "command": "cargo test" }
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &codex,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(250)).is_err(),
        "denied Codex command must not enter Testing"
    );

    let omp = serde_json::json!({
        "agent_id": "omp-agent-1",
        "workspace_id": runtime.workspace_id,
        "cwd": repo_root,
        "tool_name": "functions.bash",
        "tool_input": { "command": "cargo test" }
    })
    .to_string();
    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "pre-tool-use"], &omp);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(250)).is_err(),
        "denied OMP command must not enter Testing"
    );
}

#[test]
fn codex_permitted_tests_emit_typed_results_and_heartbeats() {
    for success in [true, false] {
        let temp = tempfile::tempdir().expect("temp dir should create");
        let paths = GlobalPaths::new(temp.path().join("home"));
        let repo_root = temp.path().join("repo");
        enable_test_repo(&paths, &repo_root);
        let (runtime, rx) = spawn_v2_started_deny_server();
        write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
        let command = format!(
            "{} sandbox run --fs build --network enabled --write-dir target --command 'python -m pytest'",
            env!("CARGO_BIN_EXE_stateful")
        );
        let pre = serde_json::json!({
            "agent_id": "codex-agent-1",
            "cwd": repo_root,
            "tool_name": "Bash",
            "tool_input": { "command": command }
        })
        .to_string();
        let output =
            run_hook_subprocess(&repo_root, &paths, &["hook", "codex", "pre-tool-use"], &pre);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let start_request = rx.recv_timeout(Duration::from_secs(2)).unwrap_or_else(|_| {
            panic!(
                "testing start should arrive; stdout: {}; stderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        let start = request_json_body(&start_request);
        assert_eq!(start["payload"]["kind"], "tool_start");
        assert_eq!(start["payload"]["tool_name"], "pytest");

        let post = serde_json::json!({
            "agent_id": "codex-agent-1",
            "cwd": repo_root,
            "tool_name": "Bash",
            "success": success,
            "tool_input": { "command": command }
        })
        .to_string();
        let output = run_hook_subprocess(
            &repo_root,
            &paths,
            &["hook", "codex", "post-tool-use"],
            &post,
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let requests = (0..2)
            .map(|_| {
                request_json_body(
                    &rx.recv_timeout(Duration::from_secs(2))
                        .expect("testing result and heartbeat should arrive"),
                )
            })
            .collect::<Vec<_>>();
        assert!(requests.iter().any(|body| {
            body["payload"]["kind"] == "tool_result"
                && body["payload"]["tool_name"] == "pytest"
                && body["payload"]["outcome"] == if success { "succeeded" } else { "failed" }
        }));
        assert!(
            requests
                .iter()
                .any(|body| body["payload"]["kind"] == "heartbeat")
        );
    }
}

#[test]
fn omp_permitted_tests_emit_typed_results_and_heartbeats() {
    for success in [true, false] {
        let temp = tempfile::tempdir().expect("temp dir should create");
        let paths = GlobalPaths::new(temp.path().join("home"));
        let repo_root = temp.path().join("repo");
        enable_test_repo(&paths, &repo_root);
        let (runtime, rx) = spawn_v2_started_deny_server();
        write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
        let command = format!(
            "{} sandbox run --fs build --network enabled --write-dir target --command 'python -m pytest'",
            env!("CARGO_BIN_EXE_stateful")
        );
        let pre = serde_json::json!({
            "agent_id": "omp-agent-1",
            "workspace_id": runtime.workspace_id,
            "cwd": repo_root,
            "tool_name": "functions.bash",
            "tool_input": { "command": command }
        })
        .to_string();
        let output =
            run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "pre-tool-use"], &pre);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let start = request_json_body(
            &rx.recv_timeout(Duration::from_secs(2))
                .expect("testing start should arrive"),
        );
        assert_eq!(start["payload"]["kind"], "tool_start");
        assert_eq!(start["payload"]["tool_name"], "pytest");

        let post = serde_json::json!({
            "agent_id": "omp-agent-1",
            "workspace_id": runtime.workspace_id,
            "cwd": repo_root,
            "tool_name": "functions.bash",
            "success": success,
            "tool_input": { "command": command }
        })
        .to_string();
        let output =
            run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "post-tool-use"], &post);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let requests = (0..2)
            .map(|_| {
                request_json_body(
                    &rx.recv_timeout(Duration::from_secs(2))
                        .expect("testing result and heartbeat should arrive"),
                )
            })
            .collect::<Vec<_>>();
        assert!(requests.iter().any(|body| {
            body["payload"]["kind"] == "tool_result"
                && body["payload"]["tool_name"] == "pytest"
                && body["payload"]["outcome"] == if success { "succeeded" } else { "failed" }
        }));
        assert!(
            requests
                .iter()
                .any(|body| body["payload"]["kind"] == "heartbeat")
        );
    }
}

#[test]
fn lost_read_start_response_queues_failed_completion_and_replays_frozen_envelopes() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo source should create");
    fs::write(repo_root.join("src/read.txt"), "contents\n").expect("read fixture should write");
    enable_test_repo(&paths, &repo_root);
    let (runtime, dropped) = spawn_fake_stateful_server_dropping_authorize();
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let input = serde_json::json!({
        "agent_id": "codex-agent-1",
        "cwd": repo_root,
        "tool_name": "functions.read",
        "tool_use_id": "read-response-loss-1",
        "tool_input": { "file_path": "src/read.txt:raw" }
    })
    .to_string();

    let pre = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );
    assert!(
        pre.status.success(),
        "{}",
        String::from_utf8_lossy(&pre.stderr)
    );
    let accepted = dropped
        .recv_timeout(Duration::from_secs(2))
        .expect("read start should reach the server before its response is lost");
    assert!(accepted.starts_with("POST /v2/read/start "));

    let outbox_file = paths.outbox_dir.join("codex-agent-1.jsonl");
    let pending = fs::read_to_string(&outbox_file)
        .expect("lost read start should remain durable")
        .lines()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).expect("outbox line should parse")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        pending.len(),
        2,
        "failed start must have a terminal completion"
    );
    assert_eq!(pending[0]["route"], "/v2/read/start");
    assert_eq!(pending[1]["route"], "/v2/read/complete");
    assert_eq!(
        accepted
            .split_once("\r\n\r\n")
            .expect("accepted request should have a body")
            .1,
        pending[0]["request_envelope"]
            .as_str()
            .expect("start request should be frozen")
    );
    for record in &pending {
        let request = record["request_envelope"]
            .as_str()
            .expect("frozen request should be a string");
        let body: serde_json::Value =
            serde_json::from_str(request).expect("frozen request should parse");
        assert_eq!(body["payload"]["operation_id"], "read-response-loss-1");
    }
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            pending[1]["request_envelope"]
                .as_str()
                .expect("completion request should be frozen"),
        )
        .expect("completion request should parse")["payload"]["classification"],
        "failed"
    );

    let (replay_runtime, replayed) =
        spawn_fake_stateful_server_sequence(vec![r#"{"status":"ok"}"#, r#"{"status":"ok"}"#]);
    assert_eq!(
        sync_outbox_with_runtime(&paths, &replay_runtime).expect("restart should replay both"),
        2
    );
    for record in &pending {
        let request = replayed
            .recv_timeout(Duration::from_secs(2))
            .expect("frozen lifecycle request should replay");
        let body = request
            .split_once("\r\n\r\n")
            .expect("replayed request should have a body")
            .1;
        assert_eq!(
            body,
            record["request_envelope"]
                .as_str()
                .expect("frozen request should be a string")
        );
    }
    assert!(
        !outbox_file.exists(),
        "successful replay must remove start and failed completion together"
    );
}

#[test]
fn lost_read_complete_response_replays_the_frozen_completion_after_restart() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo source should create");
    fs::write(repo_root.join("src/read.txt"), "contents\n").expect("read fixture should write");
    enable_test_repo(&paths, &repo_root);
    let (runtime, received) = spawn_server_dropping_route("POST /v2/read/complete ");
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let input = serde_json::json!({
        "agent_id": "codex-agent-1",
        "cwd": repo_root,
        "tool_name": "functions.read",
        "tool_use_id": "read-complete-response-loss-1",
        "tool_input": { "file_path": "src/read.txt:raw" }
    })
    .to_string();
    let pre = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );
    assert!(
        pre.status.success(),
        "{}",
        String::from_utf8_lossy(&pre.stderr)
    );
    assert!(
        received
            .recv_timeout(Duration::from_secs(2))
            .expect("read start should arrive")
            .starts_with("POST /v2/read/start ")
    );

    let post = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "post-tool-use"],
        &input,
    );
    assert!(
        post.status.success(),
        "{}",
        String::from_utf8_lossy(&post.stderr)
    );
    let accepted = received
        .recv_timeout(Duration::from_secs(2))
        .expect("read completion should reach the server before its response is lost");
    assert!(accepted.starts_with("POST /v2/read/complete "));

    let outbox_file = paths.outbox_dir.join("codex-agent-1.jsonl");
    let pending = fs::read_to_string(&outbox_file)
        .expect("lost completion should remain durable")
        .lines()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).expect("outbox line should parse")
        })
        .collect::<Vec<_>>();
    let frozen = pending
        .iter()
        .find(|record| record["route"] == "/v2/read/complete")
        .expect("read completion should be queued")["request_envelope"]
        .as_str()
        .expect("completion should be frozen")
        .to_string();
    assert_eq!(
        accepted
            .split_once("\r\n\r\n")
            .expect("accepted completion should have a body")
            .1,
        frozen
    );

    let (replay_runtime, replayed) = spawn_fake_stateful_server_sequence(
        std::iter::repeat_n(r#"{"status":"ok"}"#, pending.len()).collect(),
    );
    assert_eq!(
        sync_outbox_with_runtime(&paths, &replay_runtime).expect("restart should replay pending"),
        pending.len()
    );
    let replayed = (0..pending.len())
        .map(|_| {
            replayed
                .recv_timeout(Duration::from_secs(2))
                .expect("pending request should replay")
        })
        .collect::<Vec<_>>();
    assert!(replayed.iter().any(|request| {
        request
            .split_once("\r\n\r\n")
            .is_some_and(|(_, body)| body == frozen)
    }));
    assert!(
        !outbox_file.exists(),
        "replayed completion should be acknowledged"
    );
}

#[test]
fn lost_stop_response_replays_the_frozen_finalize_after_restart() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    enable_test_repo(&paths, &repo_root);
    let (runtime, received) = spawn_server_dropping_route("POST /v2/activity/finalize ");
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let input = r#"{"agent_id":"codex-agent-1","hook_event_name":"Stop"}"#;

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "codex", "stop"], input);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let accepted = received
        .recv_timeout(Duration::from_secs(2))
        .expect("finalize should reach the server before its response is lost");
    assert!(accepted.starts_with("POST /v2/activity/finalize "));

    let outbox_file = paths.outbox_dir.join("codex-agent-1.jsonl");
    let pending = fs::read_to_string(&outbox_file)
        .expect("lost finalize should remain durable")
        .lines()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).expect("outbox line should parse")
        })
        .collect::<Vec<_>>();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["route"], "/v2/activity/finalize");
    let frozen = pending[0]["request_envelope"]
        .as_str()
        .expect("finalize should be frozen");
    assert_eq!(
        accepted
            .split_once("\r\n\r\n")
            .expect("accepted finalize should have a body")
            .1,
        frozen
    );

    let (replay_runtime, replayed) =
        spawn_fake_stateful_server_sequence(vec![r#"{"status":"ok"}"#]);
    assert_eq!(
        sync_outbox_with_runtime(&paths, &replay_runtime).expect("restart should replay finalize"),
        1
    );
    let replayed = replayed
        .recv_timeout(Duration::from_secs(2))
        .expect("finalize should replay");
    assert_eq!(
        replayed
            .split_once("\r\n\r\n")
            .expect("replayed finalize should have a body")
            .1,
        frozen
    );
    assert!(
        !outbox_file.exists(),
        "replayed finalize should be acknowledged"
    );
}

#[test]
fn pre_write_returns_intent_and_post_success_commits_it() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo source should create");
    fs::write(repo_root.join("src/write.txt"), "before\n").expect("write fixture should create");
    enable_test_repo(&paths, &repo_root);
    let identity = r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#;
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        identity,
        identity,
        r#"{"intent_id":"intent-1","fence_ids":["fence-1"],"decision":{"decision":"allow","reason_code":"allowed","message":"ok"}}"#,
        identity,
        identity,
        r#"{"intent_id":"intent-1","outcome":"committed"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let input = serde_json::json!({
        "agent_id": "codex-agent-1",
        "cwd": repo_root,
        "tool_name": "Write",
        "call_id": "write-call-1",
        "tool_input": { "file_path": "src/write.txt" }
    })
    .to_string();
    let pre = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );
    assert!(
        pre.status.success(),
        "{}",
        String::from_utf8_lossy(&pre.stderr)
    );
    for _ in 0..2 {
        assert!(
            rx.recv()
                .expect("pre-write identity request should arrive")
                .contains("GET /v2/runtime/identity?")
        );
    }
    let authorize = rx.recv().expect("pre-write authorization should arrive");
    assert!(
        authorize.contains("POST /v2/authorize HTTP/1.1"),
        "{authorize}"
    );
    assert_eq!(
        request_json_body(&authorize)["payload"]["operation_id"],
        "write-call-1"
    );

    fs::write(repo_root.join("src/write.txt"), "after\n").expect("write fixture should update");
    let post = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "post-tool-use"],
        &input,
    );
    assert!(
        post.status.success(),
        "{}",
        String::from_utf8_lossy(&post.stderr)
    );
    for _ in 0..2 {
        assert!(
            rx.recv()
                .expect("post-write identity request should arrive")
                .contains("GET /v2/runtime/identity?")
        );
    }
    let complete = rx.recv().expect("write completion should arrive");
    assert!(
        complete.contains("POST /v2/write/complete HTTP/1.1"),
        "{complete}"
    );
    let body = request_json_body(&complete);
    assert_eq!(body["payload"]["intent_id"], "intent-1");
    assert_eq!(body["payload"]["outcome"], "committed");
    assert!(body["payload"]["post_fingerprints"][0][1]["sha256"].is_string());
}

#[test]
fn denied_v2_write_intent_completes_the_exact_started_intent_as_failed() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo source should create");
    fs::write(repo_root.join("src/deny.txt"), "before\n").expect("write fixture should create");
    enable_test_repo(&paths, &repo_root);
    let (runtime, requests) = spawn_v2_started_deny_server();
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let input = serde_json::json!({
        "agent_id": "codex-agent-1",
        "cwd": repo_root,
        "tool_name": "Write",
        "tool_use_id": "deny-operation-1",
        "tool_input": { "file_path": "src/deny.txt", "content": "after" }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rendered: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("denial should serialize");
    assert_eq!(rendered["hookSpecificOutput"]["permissionDecision"], "deny");

    let authorize = request_json_body(
        &requests
            .recv_timeout(Duration::from_secs(2))
            .expect("authorization should arrive"),
    );
    assert_eq!(authorize["payload"]["operation_id"], "deny-operation-1");
    let completion = request_json_body(
        &requests
            .recv_timeout(Duration::from_secs(2))
            .expect("failed completion should arrive"),
    );
    assert_eq!(completion["payload"]["intent_id"], "intent-denied-1");
    assert_eq!(completion["payload"]["outcome"], "failed");
}

#[test]
fn session_start_registers_explicit_agent_without_current_file() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "agent_id": "codex-agent-1",
      "transcript_path": "/tmp/transcript.jsonl",
      "cwd": "/repo",
      "hook_event_name": "SessionStart"
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "session-start"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("session register request should arrive");
    assert!(request.contains("POST /v2/session/register HTTP/1.1"));
    assert!(request.contains("\"agent_id\":\"codex-agent-1\""));
    assert_current_agent_context_absent(&repo_root);
}

#[test]
fn session_start_derives_workspace_id_for_default_local_runtime() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (mut runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    runtime.workspace_id = "local".to_string();
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "agent_id": "derived-agent",
      "transcript_path": "/tmp/transcript.jsonl",
      "cwd": "/repo",
      "hook_event_name": "SessionStart"
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "session-start"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("session register request should arrive");
    let body = request_json_body(&request);
    let workspace_id = body["workspace"]["workspace_id"]
        .as_str()
        .expect("workspace_id should be a string");
    assert!(workspace_id.starts_with("workspace-"));
    assert_ne!(workspace_id, "local");

    assert_eq!(body["agent"]["agent_id"], "derived-agent");
    assert_current_agent_context_absent(&repo_root);
}

#[test]
fn session_start_derives_workspace_id_for_default_shared_runtime() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (mut runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    runtime.workspace_id = "shared".to_string();
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "agent_id": "derived-shared-agent",
      "transcript_path": "/tmp/transcript.jsonl",
      "cwd": "/repo",
      "hook_event_name": "SessionStart"
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "session-start"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("session register request should arrive");
    let body = request_json_body(&request);
    let workspace_id = body["workspace"]["workspace_id"]
        .as_str()
        .expect("workspace_id should be a string");
    assert!(workspace_id.starts_with("workspace-"));
    assert_ne!(workspace_id, "shared");

    assert_eq!(body["agent"]["agent_id"], "derived-shared-agent");
    assert_current_agent_context_absent(&repo_root);
}

#[test]
fn pre_tool_use_authorization_uses_explicit_agent_id_when_thread_id_present() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) =
        spawn_fake_stateful_server(r#"{"status":"ok","prompt_text":"Nearby Activity\n- none"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "agent_id": "parent-session-1",
      "thread_id": "codex-thread-1",
      "transcript_path": "/tmp/transcript.jsonl",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    let body = request_json_body(&request);
    assert_eq!(body["agent"]["agent_id"], "parent-session-1");
}

#[test]
fn pre_tool_use_denies_raw_read_only_bash_after_sandbox_runner_migration() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg auth src"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "stateful sandbox run");
}

#[test]
fn pre_tool_use_denies_namespaced_raw_bash_tool() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "functions.bash",
      "tool_input": {
        "command": "pwd"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "stateful sandbox run");
}

#[test]
fn pre_tool_use_allows_namespaced_safe_read_tool() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "functions.read",
      "tool_input": {
        "path": "README.md"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_raw_bash_denial_mentions_command_policy_skill_and_example() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "git status"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");
    let HookOutcome::Deny { reason } = outcome else {
        panic!("raw Bash should be denied");
    };

    assert!(reason.contains("stateful-command-policy"));
    assert!(reason.contains("state_context_render"));
    assert!(reason.contains("planning/manual inspection"));
    assert!(reason.contains("state_reservation_declare"));
    assert!(reason.contains("state_claim_acquire"));
    assert!(reason.contains("only when they appear in the tool list"));
    assert!(reason.contains("lazy resume helpers"));
    assert!(reason.contains("--fs read-only --network disabled"));
    assert!(reason.contains("--fs build --network enabled"));
    assert!(reason.contains("--fs write-targets --write-target <file>"));
    assert!(reason.contains("--command"));
}

#[test]
fn pre_tool_use_requires_read_only_sandbox_for_shell_read_fallback() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "cat README.md"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_bash_denial_mentions_all(
        outcome,
        &[
            "Raw Bash is denied",
            "stateful-command-policy",
            "--fs read-only --network disabled",
        ],
    );
}

#[test]
fn pre_tool_use_allows_sandbox_external_for_repo_external_write_approval_path() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs external --purpose 'write external artifact' --write-target /tmp/stateful-outside.txt --command 'printf ok > /tmp/stateful-outside.txt'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_sandbox_external_without_write_scope() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs external --purpose 'inspect external environment' --command 'pwd'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_sandbox_external_sequence() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs external --purpose 'launch benchmark' --write-dir /tmp/stateful-bench --sequence-shell /bin/zsh --sequence 'set -euo pipefail' --sequence 'printf ok > /tmp/stateful-bench/out'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_denies_sandbox_sequence_with_command() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --command 'printf ok' --sequence 'printf later'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(
        outcome,
        "stateful sandbox run accepts either --command or --sequence, not both",
    );
}

#[test]
fn pre_tool_use_denies_git_profile_sequence() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs git --network disabled --sequence 'git status'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "git profile requires a single git command");
}

#[test]
fn pre_tool_use_allows_sandbox_external_with_supported_scopes() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs external --purpose 'update external artifacts' --network enabled --write-target /tmp/stateful-existing.txt --create-target /tmp/stateful-new.txt --write-dir /tmp/stateful-dir --connect-socket /tmp/stateful.sock --allow-signal --command 'printf ok'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_denies_sandbox_external_without_prompt_matched_prefix() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --purpose 'write external artifact' --fs external --write-target /tmp/stateful-outside.txt --command 'printf ok > /tmp/stateful-outside.txt'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "canonical prompt-matched prefix");
}

#[test]
fn pre_tool_use_denies_sandbox_external_without_purpose() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs external --write-target /tmp/stateful-outside.txt --command 'printf ok > /tmp/stateful-outside.txt'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "external sandbox profile requires --purpose");
}

#[test]
fn pre_tool_use_denies_external_run_request_for_repo_external_write() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} external-run request --purpose 'write external artifact' --write-target /tmp/stateful-outside.txt --command 'printf ok > /tmp/stateful-outside.txt'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "stateful sandbox run");
}

#[test]
fn pre_tool_use_denies_raw_repo_external_write_without_approval_wrapper() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "printf ok > /tmp/stateful-outside.txt"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_bash_denial_mentions_all(
        outcome,
        &[
            "Raw Bash is denied",
            "stateful sandbox run",
            "stateful-command-policy",
        ],
    );
}

#[test]
fn pre_tool_use_allows_canonical_sandbox_run_read_only() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --network disabled --timeout-seconds 30 --command 'rg auth src'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_shell_escaped_nested_command_quotes() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs git --network disabled --command 'git commit -m '\\''docs: clarify methodology validation boundaries'\\'''")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_structured_process_find() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox process find --contains denovo_codex_agent")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_structured_process_find_json_envelope() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox process find --json --contains denovo_codex_agent")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_denies_process_find_without_selector() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox process find")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(
        outcome,
        "stateful sandbox process find requires at least one selector",
    );
}

#[test]
fn pre_tool_use_denies_sandbox_run_with_raw_process_inspection() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --network disabled --command 'pgrep -f denovo_codex_agent'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(
        outcome,
        "process inspection must use stateful sandbox process find",
    );
}

#[test]
fn pre_tool_use_denies_sandbox_run_with_wrapped_raw_process_inspection() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --network disabled --command 'env -i pgrep -f denovo_codex_agent'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(
        outcome,
        "process inspection must use stateful sandbox process find",
    );
}

#[test]
fn pre_tool_use_denies_untrusted_process_find() {
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": "/bin/echo sandbox process find --contains denovo_codex_agent"
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(
        outcome,
        "stateful sandbox process find requires a trusted stateful binary",
    );
}

#[test]
fn pre_tool_use_denies_read_only_sandbox_run_with_network_enabled() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --network enabled --command 'curl https://example.test'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "read-only sandbox run requires --network disabled");
}

#[test]
fn pre_tool_use_allows_canonical_sandbox_run_write_targets() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs write-targets --network enabled --write-target README.md --command 'printf x > README.md'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_sandbox_run_git_profile_for_git_commands() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs git --network enabled --timeout-seconds 30 --command 'git fetch --all'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_sandbox_run_github_pr_profile_for_pr_commands() {
    let stateful = trusted_stateful_path();
    for command in [
        "gh pr list",
        "gh pr view 123 --json title,url",
        "gh pr status",
        "gh pr create --title 'Update policy' --body 'Adds github-pr profile' --base dev --head feature --draft",
    ] {
        let input = serde_json::json!({
            "agent_id": "s1",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {
                "command": format!("{stateful} sandbox run --fs github-pr --network enabled --timeout-seconds 30 --command {command:?}")
            }
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");
        assert_eq!(
            outcome,
            HookOutcome::Allow,
            "expected `{command}` to be allowed"
        );
    }
}

#[test]
fn pre_tool_use_denies_sandbox_run_github_pr_profile_unsafe_requests() {
    let stateful = trusted_stateful_path();
    let cases = [
        (
            "network disabled",
            format!(
                "{stateful} sandbox run --fs github-pr --network disabled --command 'gh pr status'"
            ),
            "github-pr sandbox run requires --network enabled",
        ),
        (
            "explicit write target",
            format!(
                "{stateful} sandbox run --fs github-pr --network enabled --write-target README.md --command 'gh pr status'"
            ),
            "github-pr profile manages transient PR state automatically",
        ),
        (
            "non pr gh command",
            format!(
                "{stateful} sandbox run --fs github-pr --network enabled --command 'gh auth status'"
            ),
            "github-pr profile requires a single gh pr command",
        ),
        (
            "merge",
            format!(
                "{stateful} sandbox run --fs github-pr --network enabled --command 'gh pr merge 123'"
            ),
            "github-pr profile does not allow gh pr subcommand `merge`",
        ),
        (
            "web",
            format!(
                "{stateful} sandbox run --fs github-pr --network enabled --command 'gh pr create --web'"
            ),
            "github-pr profile does not allow interactive/browser flag `--web`",
        ),
    ];

    for (name, command, expected) in cases {
        let input = serde_json::json!({
            "agent_id": "s1",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {
                "command": command
            }
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");
        let HookOutcome::Deny { reason } = outcome else {
            panic!("{name}: expected Bash denial");
        };
        assert!(
            reason.contains(expected),
            "{name}: reason `{reason}` should contain `{expected}`"
        );
    }
}

#[test]
fn pre_tool_use_denies_sandbox_run_git_profile_dispatch_surfaces() {
    let stateful = trusted_stateful_path();
    let cases = [
        (
            "alias config",
            format!(
                "{stateful} sandbox run --fs git --network enabled --command \"git -c alias.x='!curl https://example.test' x\""
            ),
        ),
        (
            "config env",
            format!(
                "{stateful} sandbox run --fs git --network enabled --command 'git --config-env=alias.x=STATEFUL_ALIAS x'"
            ),
        ),
        (
            "submodule foreach",
            format!(
                "{stateful} sandbox run --fs git --network enabled --command \"git submodule foreach 'printf nope'\""
            ),
        ),
        (
            "rebase exec",
            format!(
                "{stateful} sandbox run --fs git --network enabled --command \"git rebase --exec 'printf nope'\""
            ),
        ),
        (
            "grep pager",
            format!(
                "{stateful} sandbox run --fs git --network enabled --command \"git grep --open-files-in-pager='sh -c id' TODO\""
            ),
        ),
        (
            "remote mutation",
            format!(
                "{stateful} sandbox run --fs git --network enabled --command 'git remote add origin https://example.test/repo.git'"
            ),
        ),
    ];

    for (name, command) in cases {
        let input = serde_json::json!({
            "agent_id": "s1",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {
                "command": command
            }
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

        let HookOutcome::Deny { reason } = outcome else {
            panic!("{name}: expected Bash denial");
        };
        assert!(
            reason.contains("git profile"),
            "{name}: reason `{reason}` should mention git profile"
        );
    }
}

#[test]
fn pre_tool_use_allows_sandbox_run_write_dir() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs write-targets --network enabled --write-dir tmp/run-1 --command 'cargo test'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_sandbox_run_direct_tmp_write_dir() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs write-targets --network enabled --write-dir tmp --command 'cargo test'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_sandbox_run_write_dir_outside_artifact_tree() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs write-targets --network enabled --write-dir target --command 'cargo test'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_allows_sandbox_run_build_profile_with_scoped_tmp_write_dir() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs build --network enabled --write-dir build-tests --command 'npm test'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
fn pre_tool_use_denies_sandbox_run_build_profile_without_scoped_tmp_write_dir() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs build --network enabled --command 'npm test'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "--write-dir <scratch-purpose>");
}

#[test]
#[cfg(not(feature = "codex-benchmark"))]
fn pre_tool_use_denies_nested_codex_benchmark_sandbox_without_feature() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!(
                "{stateful} sandbox run-nested-codex-benchmark --purpose 'run nested Codex chaos benchmark' --write-dir target --codex-home-root target/nested-codex-homes/run-1 --timeout-seconds 120 --command 'cargo run -p stateful-bench -- run'"
            )
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(
        outcome,
        "run-nested-codex-benchmark hook authorization requires the codex-benchmark feature",
    );
}

#[test]
#[cfg(feature = "codex-benchmark")]
fn pre_tool_use_allows_nested_codex_benchmark_sandbox_with_feature() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!(
                "{stateful} sandbox run-nested-codex-benchmark --purpose 'run nested Codex chaos benchmark' --write-dir target --codex-home-root target/nested-codex-homes/run-1 --timeout-seconds 120 --command 'cargo run -p stateful-bench -- run'"
            )
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
#[cfg(feature = "codex-benchmark")]
fn pre_tool_use_allows_nested_codex_benchmark_sandbox_with_docker_socket() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!(
                "{stateful} sandbox run-nested-codex-benchmark --purpose bench --write-dir target --codex-home-root target/nested-codex-homes/run-1 --docker-socket /private/tmp/docker.sock --command true"
            )
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
#[cfg(feature = "codex-benchmark")]
fn pre_tool_use_allows_tmux_new_session_for_nested_codex_benchmark() {
    let stateful = trusted_stateful_path();
    let tmux = trusted_tmux_path();
    let nested = format!(
        "{stateful} sandbox run-nested-codex-benchmark --purpose 'run DeNovoSWE full dataset Codex benchmark fixture a' --write-dir target --codex-home-root target/nested-codex-homes/fixture-denovo-full-3-codex-a --docker-socket /private/tmp/docker.sock --timeout-seconds 43200 --command 'target/debug/stateful-bench denovo run --run-id fixture-denovo-full-3-codex-a'"
    );
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!(
                "{tmux} new-session -d -s fixture-denovo-full-3-codex-a -c /repo \"{nested}\""
            )
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_eq!(outcome, HookOutcome::Allow);
}

#[test]
#[cfg(feature = "codex-benchmark")]
fn pre_tool_use_denies_tmux_send_keys_even_for_benchmark_agents() {
    let tmux = trusted_tmux_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!(
                "{tmux} send-keys -t fixture-denovo-full-3-codex-a 'rm -rf target' C-m"
            )
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "tmux benchmark launcher supports only new-session");
}

#[test]
fn pre_tool_use_denies_external_run_request() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} external-run request --purpose 'install rebuilt binaries' --write-dir /opt/stateful/bin --command 'install -m 755 target/release/stateful /opt/stateful/bin/stateful'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "stateful sandbox run");
}

#[test]
fn pre_tool_use_denies_external_run_approve_and_run() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} external-run approve request-123 --run")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "stateful sandbox run");
}

#[test]
fn pre_tool_use_denies_external_run_run() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} external-run run request-123")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "stateful sandbox run");
}

#[test]
fn pre_tool_use_denies_trusted_stateful_git_wrappers() {
    let stateful = trusted_stateful_path();
    let cases = [
        format!("{stateful} commit -m docs-update -- README.md"),
        format!("{stateful} pull"),
        format!("{stateful} push origin dev"),
    ];

    for command in cases {
        let input = serde_json::json!({
            "agent_id": "s1",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {
                "command": command
            }
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

        assert_bash_denial_mentions(outcome, "stateful sandbox run");
    }
}

#[test]
fn pre_tool_use_allows_trusted_stateful_server_control() {
    let stateful = trusted_stateful_path();
    let cases = [
        format!("{stateful} server stop"),
        format!("{stateful} server start"),
    ];

    for command in cases {
        let input = serde_json::json!({
            "agent_id": "s1",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {
                "command": command
            }
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

        assert_eq!(outcome, HookOutcome::Allow);
    }
}

#[test]
fn pre_tool_use_denies_external_run_with_outer_command_separator() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} external-run approve request-123 --run; rm README.md")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "single stateful sandbox run command");
}

#[test]
fn pre_tool_use_denies_sandbox_run_with_outer_command_separator() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --command 'rg auth src'; rm README.md")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "single stateful sandbox run command");
}

#[test]
fn pre_tool_use_denies_sandbox_run_write_targets_without_target() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs write-targets --network enabled --command 'printf x > README.md'")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(
        outcome,
        "requires at least one write target, create target, or write dir",
    );
}

#[test]
#[cfg(feature = "codex-benchmark")]
fn pre_tool_use_denies_invalid_nested_codex_benchmark_sandbox_wrappers() {
    let stateful = trusted_stateful_path();
    let cases = [
        (
            "missing purpose",
            format!(
                "{stateful} sandbox run-nested-codex-benchmark --write-dir target --codex-home-root target/nested-codex-homes/run-1 --command 'cargo test'"
            ),
            "requires --purpose",
        ),
        (
            "missing home root",
            format!(
                "{stateful} sandbox run-nested-codex-benchmark --purpose 'run nested Codex chaos benchmark' --write-dir target --command 'cargo test'"
            ),
            "requires --codex-home-root",
        ),
        (
            "source write dir",
            format!(
                "{stateful} sandbox run-nested-codex-benchmark --purpose 'run nested Codex chaos benchmark' --write-dir crates --codex-home-root target/nested-codex-homes/run-1 --command 'cargo test'"
            ),
            "requires --write-dir target",
        ),
        (
            "home outside target",
            format!(
                "{stateful} sandbox run-nested-codex-benchmark --purpose 'run nested Codex chaos benchmark' --write-dir target --codex-home-root /opt/codex-home --command 'cargo test'"
            ),
            "requires --codex-home-root under target",
        ),
        (
            "unsupported write target",
            format!(
                "{stateful} sandbox run-nested-codex-benchmark --purpose 'run nested Codex chaos benchmark' --write-dir target --codex-home-root target/nested-codex-homes/run-1 --write-target README.md --command 'cargo test'"
            ),
            "unsupported stateful sandbox run-nested-codex-benchmark argument",
        ),
        (
            "generic relaxed profile",
            format!(
                "{stateful} sandbox run --fs relaxed --network enabled --write-dir target --command 'cargo test'"
            ),
            "supports only read-only, write-targets, external, build, git, and github-pr profiles",
        ),
        (
            "build profile with explicit write target",
            format!(
                "{stateful} sandbox run --fs build --network enabled --write-target README.md --command 'npm test'"
            ),
            "build profile rejects explicit write targets, create targets, connect sockets, and signal scope",
        ),
        (
            "git profile with non-git command",
            format!("{stateful} sandbox run --fs git --network enabled --command 'rm README.md'"),
            "git profile requires a single git command",
        ),
        (
            "git profile with shell control in inner command",
            format!(
                "{stateful} sandbox run --fs git --network enabled --command 'git status; rm README.md'"
            ),
            "git profile requires a single git command",
        ),
        (
            "git profile with explicit write target",
            format!(
                "{stateful} sandbox run --fs git --network enabled --write-target README.md --command 'git status'"
            ),
            "git profile manages repo writes automatically",
        ),
    ];

    for (name, command, expected) in cases {
        let input = serde_json::json!({
            "agent_id": "s1",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {
                "command": command
            }
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");
        assert_bash_denial_mentions(outcome, expected);
        let _ = name;
    }
}

#[test]
fn pre_tool_use_denies_sandbox_run_with_shell_escape_quote_mismatch() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --command \\\"rg; rm README.md #\\\"")
        }
    })
    .to_string();

    let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

    assert_bash_denial_mentions(outcome, "shell escapes");
}

#[test]
fn pre_tool_use_denies_invalid_sandbox_run_outer_wrappers() {
    let stateful = trusted_stateful_path();
    let cases = [
        (
            "outer env assignment",
            format!("FOO=bar {stateful} sandbox run --command 'rg auth src'"),
            "environment assignments",
        ),
        (
            "command substitution",
            format!("{stateful} sandbox run --command \"$(pwd)\""),
            "command substitution",
        ),
        (
            "outer redirect",
            format!("{stateful} sandbox run --command 'rg auth src' > /tmp/out"),
            "single stateful sandbox run command",
        ),
        (
            "outer pipeline",
            format!("{stateful} sandbox run --command 'rg auth src' | cat"),
            "single stateful sandbox run command",
        ),
        (
            "untrusted executable",
            "/bin/echo sandbox run --command 'rg auth src'".to_string(),
            "trusted stateful binary",
        ),
        (
            "duplicate command",
            format!("{stateful} sandbox run --command 'rg auth src' --command pwd"),
            "exactly one --command",
        ),
        (
            "invalid fs",
            format!("{stateful} sandbox run --fs read-write --command 'rg auth src'"),
            "supports only read-only, write-targets, external, build, git, and github-pr profiles",
        ),
        (
            "invalid network",
            format!("{stateful} sandbox run --network inherited --command 'rg auth src'"),
            "network must be disabled or enabled",
        ),
        (
            "missing option value",
            format!("{stateful} sandbox run --command 'rg auth src' --write-target"),
            "argument `--write-target` requires a value",
        ),
        (
            "missing timeout value",
            format!("{stateful} sandbox run --command 'rg auth src' --timeout-seconds"),
            "argument `--timeout-seconds` requires a value",
        ),
        (
            "non-integer timeout value",
            format!("{stateful} sandbox run --timeout-seconds nope --command 'rg auth src'"),
            "--timeout-seconds requires an integer value",
        ),
    ];

    for (name, command, expected) in cases {
        let input = serde_json::json!({
            "agent_id": "s1",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {
                "command": command
            }
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

        let HookOutcome::Deny { reason } = outcome else {
            panic!("{name}: expected Bash denial");
        };
        assert!(
            reason.contains(expected),
            "{name}: reason `{reason}` should contain `{expected}`"
        );
    }
}

#[test]
fn pre_tool_use_denies_raw_read_only_bash_with_sandbox_run_guidance() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg auth src"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_bash_even_when_legacy_sandbox_metadata_exists() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": false
      },
      "tool_input": {
        "command": "rg auth src"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_quoted_rg_regex_alternation() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg -n \"future work|Future work\" docs crates README.md .stateful"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_read_only_bash_dev_null_redirection() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg -n \"future work\" docs 2>/dev/null"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_test_bash_with_sandbox_run_guidance() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "cargo test"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_stateful_diagnostic_bash() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "./target/debug/stateful doctor"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_stateful_bench_operational_bash() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "target/debug/stateful-bench run --pairs .stateful_bench/pairs/all.jsonl --mode no-state --agent-cmd-template codex"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_in_repo_does_not_write_current_agent_context() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, _rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "agent_id": "s-current",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg auth src"
      }
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_current_agent_context_absent(&repo_root);
}

#[test]
fn pre_tool_use_uses_payload_agent_id_without_environment_fallback() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "agent_id": "s-current",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("authorize request should arrive");
    assert!(request.contains("\"agent_id\":\"s-current\""));
    assert_current_agent_context_absent(&repo_root);
}

#[test]
fn pre_tool_use_from_enabled_subdir_records_session_at_repo_root() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    let subdir = repo_root.join("nested/worktree");
    fs::create_dir_all(&subdir).expect("subdir should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "agent_id": "s-subdir",
      "cwd": "/repo/nested/worktree",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let output = run_hook_subprocess(&subdir, &paths, &["hook", "codex", "pre-tool-use"], input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _request = rx.recv().expect("captured request should arrive");
    assert_current_agent_context_absent(&repo_root);
    assert!(!subdir.join(".stateful_core/runtime/session.json").exists());
}

#[test]
fn pre_tool_use_in_disabled_repo_noops_without_runtime() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    fs::create_dir_all(temp_root.join(".git")).expect("git marker should write");

    let input = r#"{
      "agent_id": "s-disabled",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let outcome =
        handle_pre_tool_use_in_repo(input, temp_root).expect("disabled repo should no-op");

    assert_eq!(outcome, HookOutcome::Allow);
    assert!(
        !temp_root
            .join(".stateful_core/runtime/session.json")
            .exists()
    );
}

#[test]
fn pre_tool_use_allows_read_only_sandbox_when_runtime_unreachable() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let stateful = env!("CARGO_BIN_EXE_stateful");
    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": "/repo",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --network disabled --command 'rg auth src'")
        }
    })
    .to_string();

    let output = run_hook_subprocess_with_extra_env(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
        &[
            ("STATEFUL_SERVER_URL", "http://127.0.0.1:9"),
            ("STATEFUL_SERVER_TOKEN", "unreachable-token"),
        ],
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "allowed hook should not print a denial: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn pre_tool_use_denies_native_write_when_runtime_unreachable() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_use_id": "unreachable-write",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let output = run_hook_subprocess_with_extra_env(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
        &[
            ("STATEFUL_SERVER_URL", "http://127.0.0.1:9"),
            ("STATEFUL_SERVER_TOKEN", "unreachable-token"),
        ],
    );

    assert!(
        output.status.success(),
        "stateful hook should return a structured denial, not crash: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"permissionDecision\":\"deny\""));
    assert!(stdout.contains("reachable stateful.v2 server"));
}

#[test]
fn pre_tool_use_denies_raw_stateful_reservation_declare_with_native_tool_guidance() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
      "command": "stateful reservation declare --agent-id s1 --workspace-id w1 --purpose 'Fix auth validation behavior.' src/auth.ts"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_bash_denial_mentions_all(
        outcome,
        &[
            "Use active Stateful native coordination tools only when they appear in the tool list",
            "state_reservation_declare",
            "state_claim_acquire",
            "lazy resume helpers",
            "Do not run `stateful reservation declare`",
        ],
    );
}

#[test]
fn pre_tool_use_denies_legacy_stateful_mcp_call_with_native_tool_guidance() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "stateful mcp call state_reservation_declare"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_bash_denial_mentions_all(
        outcome,
        &[
            "Use active Stateful native coordination tools only when they appear in the tool list",
            "state_reservation_declare",
            "state_claim_acquire",
            "lazy resume helpers",
            "legacy `stateful mcp call` through Bash",
        ],
    );
}

#[test]
fn pre_tool_use_denies_raw_other_stateful_control_commands() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "stateful sync-outbox"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_bash_control_syntax_even_with_legacy_read_only_tmp_sandbox() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": false
      },
      "tool_input": {
        "command": "rg auth src | head > /tmp/stateful-rg.out"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_pipeline_even_with_legacy_read_only_tmp_sandbox() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": false
      },
      "tool_input": {
        "command": "rg -n \"future work|Future work\" docs | head > /tmp/stateful-rg.out"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_bash_control_syntax_without_sandbox_run_wrapper() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg auth src | head > /tmp/stateful-rg.out"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn run_hook_pre_tool_use_denies_raw_bash_with_legacy_trusted_sandbox_env() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg auth src | head > /tmp/stateful-rg.out"
      }
    }"#;
    let trusted_sandbox = serde_json::json!({
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": false,
        "source": "stateful-codex-wrapper"
    })
    .to_string();

    let output = run_hook_subprocess_with_extra_env(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
        &[("STATEFUL_HOOK_TRUSTED_SANDBOX", trusted_sandbox.as_str())],
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        json["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .expect("reason should be string")
            .contains("stateful sandbox run")
    );
}

#[test]
fn pre_tool_use_denies_tool_input_spoofed_read_only_bash_sandbox() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "tool_input": {
        "command": "rg auth src | head",
        "sandbox": {
          "mode": "read-only",
          "writable_roots": ["/tmp"],
          "network_access": false
        }
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_bash_even_with_legacy_repo_writable_root_metadata() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/repo"],
        "network_access": false
      },
      "tool_input": {
        "command": "rg auth src | head"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_bash_even_with_legacy_network_access_metadata() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": true
      },
      "tool_input": {
        "command": "rg auth src | head"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_bash_even_with_incomplete_legacy_network_metadata() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"]
      },
      "tool_input": {
        "command": "rg auth src | head"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_mutating_command_even_with_legacy_read_only_sandbox() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": false
      },
      "tool_input": {
        "command": "rm /tmp/stateful-rg.out"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_raw_stateful_control_command_even_with_legacy_read_only_sandbox() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": false
      },
      "tool_input": {
        "command": "stateful sync-outbox"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_arbitrary_raw_bash_even_with_legacy_read_only_sandbox() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "Bash",
      "sandbox": {
        "mode": "read-only",
        "writable_roots": ["/tmp"],
        "network_access": false
      },
      "tool_input": {
        "command": "echo hi > src/auth.ts; stateful sync-outbox || true"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert_raw_bash_denied_with_sandbox_run_guidance(outcome);
}

#[test]
fn pre_tool_use_denies_apply_patch_until_intent_protocol_exists() {
    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let outcome = handle_pre_tool_use(input).expect("hook input should parse");

    assert!(matches!(outcome, HookOutcome::Deny { .. }));
}

#[test]
fn pre_tool_use_allows_known_non_repo_write_tools_without_runtime() {
    for tool_name in [
        "Read",
        "Grep",
        "Glob",
        "LS",
        "NotebookRead",
        "WebFetch",
        "WebSearch",
        "TodoWrite",
        "update_plan",
        "tool_search",
        "tool_search_tool",
        "get_goal",
        "search_tool_bm25",
        "create_goal",
        "update_goal",
        "request_user_input",
        "view_image",
        "task",
        "spawn_agent",
        "multi_agent_v1spawn_agent",
        "wait_agent",
        "multi_agent_v1wait_agent",
        "send_input",
        "multi_agent_v1send_input",
        "close_agent",
        "multi_agent_v1close_agent",
        "resume_agent",
        "multi_agent_v1resume_agent",
        "state_reservation_declare",
        "state_claim_acquire",
        "state_current_read",
        "state_context_render",
        "state_reconcile_ack",
        "state.reconcile.ack",
        "mcp__stateful__state_reservation_declare",
        "mcp__stateful__state_claim_acquire",
        "mcp__stateful_state_reservation_declare",
        "mcp__stateful_state_claim_acquire",
        "mcp__stateful_state_current_read",
        "mcp__stateful_state_context_render",
        "mcp__stateful__state_current_read",
        "mcp__stateful__state_context_render",
        "mcp__stateful_state_reconcile_ack",
        "mcp__stateful__state_reconcile_ack",
        "mcp__codex_apps__github__update_pull_request",
        "mcp__codex_apps__github__create_pull_request",
        "mcp__codex_apps__github__add_comment_to_issue",
        "mcp__codex_apps__github__fetch_file",
        "mcp__codex_apps__github__search_branches",
        "mcp__codex_apps__github__search_repositories",
        "mcp__codex_apps__microsoft_teams__resolve_channel",
        "mcp__codex_apps__microsoft_teams__send_message",
    ] {
        let input = serde_json::json!({
            "agent_id": "s1",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": tool_name,
            "tool_input": {}
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

        assert_eq!(
            outcome,
            HookOutcome::Allow,
            "{tool_name} should be classified as safe without repo write authorization"
        );
    }
}

#[test]
fn pre_tool_use_denies_unclassified_tool_names() {
    for tool_name in [
        "FutureWriteTool",
        "mcp__codex_apps__github__merge_pull_request",
        "mcp__codex_apps__microsoft_teams__delete_message",
    ] {
        let input = serde_json::json!({
          "agent_id": "s1",
          "cwd": "/repo",
          "hook_event_name": "PreToolUse",
          "tool_name": tool_name,
          "tool_input": {
            "file_path": "src/auth.ts"
          }
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

        let HookOutcome::Deny { reason } = outcome else {
            panic!("{tool_name} should be denied");
        };
        assert!(
            reason.contains(&format!("unclassified tool {tool_name}")),
            "reason `{reason}` should name the unclassified tool"
        );
        assert!(
            reason.contains("write or execute"),
            "reason `{reason}` should explain why classification is required"
        );
    }
}

#[test]
fn pre_tool_use_bash_denial_in_repo_does_not_render_live_context() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"status":"ok","prompt_text":"Nearby Activity\n- unexpected context"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": "rg auth src"
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "Bash denial should not render live context"
    );
    let rendered: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(rendered["hookSpecificOutput"]["permissionDecision"], "deny");
    let reason = rendered["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .expect("deny reason should be text");
    assert!(reason.contains("Raw Bash is denied"));
    assert!(!reason.contains("Nearby Activity"));
}

#[test]
fn pre_tool_use_denies_github_remote_repository_mutation_tools() {
    for tool_name in [
        "mcp__codex_apps__github__create_file",
        "mcp__codex_apps__github__update_file",
        "mcp__codex_apps__github__create_branch",
        "mcp__codex_apps__github__update_ref",
    ] {
        let input = serde_json::json!({
            "agent_id": "s1",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": tool_name,
            "tool_input": {}
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

        let HookOutcome::Deny { reason } = outcome else {
            panic!("{tool_name} should be denied");
        };
        assert!(
            reason.contains("remote repository mutation"),
            "reason `{reason}` should classify {tool_name} as remote repository mutation"
        );
    }
}

#[test]
fn pre_tool_use_records_unclassified_tools_for_tools_list() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "FutureWriteTool",
        "tool_input": {}
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");

    let list = tool_list_for_repo(&paths, &repo_root).expect("tool list should load");
    assert_eq!(list.unclassified_tools, vec!["FutureWriteTool"]);
}

#[test]
fn pre_tool_use_allows_repo_tool_allowlist_but_preserves_hard_denies() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    allow_tool_for_repo(
        &paths,
        &repo_root,
        "mcp__codex_apps__github__merge_pull_request",
    )
    .expect("unclassified tool should be user-allowed");
    allow_tool_for_repo(&paths, &repo_root, "mcp__filesystem__write_file")
        .expect("hard-denied tool name can be stored but must not override hard deny");
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let allowed_input = serde_json::json!({
        "agent_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "mcp__codex_apps__github__merge_pull_request",
        "tool_input": {}
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &allowed_input,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "allowed hook outcome should not print a denial, got {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let denied_input = serde_json::json!({
        "agent_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "mcp__filesystem__write_file",
        "tool_input": {}
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &denied_input,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        json["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .expect("reason should be string")
            .contains("filesystem MCP")
    );
}

#[test]
fn pre_tool_use_denies_edit_and_write_without_runtime() {
    for (tool_name, tool_input) in [
        (
            "Edit",
            serde_json::json!({
                "file_path": "src/auth.ts",
                "old_string": "old",
                "new_string": "new"
            }),
        ),
        (
            "Write",
            serde_json::json!({
                "file_path": "src/auth.ts",
                "content": "new"
            }),
        ),
    ] {
        let input = serde_json::json!({
            "agent_id": "s1",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": tool_name,
            "tool_input": tool_input
        })
        .to_string();

        let outcome = handle_pre_tool_use(&input).expect("hook input should parse");

        assert!(
            matches!(outcome, HookOutcome::Deny { .. }),
            "{tool_name} should require stateful authorization"
        );
    }
}

#[test]
fn omp_write_authorize_records_runtime_lineage_without_commit_policy_input() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(repo_root.join("docs")).expect("repo docs should create");
    fs::write(repo_root.join("docs/a.md"), "before\n").expect("fixture should write");
    enable_test_repo(&paths, &repo_root);
    let identity = r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#;
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        identity,
        identity,
        r#"{"intent_id":"intent-1","decision":{"decision":"deny","reason_code":"active_claim_conflict","message":"blocked"}}"#,
        r#"{"status":"completed"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("runtime file should write");
    let input = serde_json::json!({
        "runtime": "omp",
        "agent_id": "omp-parent",
        "parent_agent_id": serde_json::Value::Null,
        "omp_agent_id": "main",
        "workspace_id": runtime.workspace_id,
        "cwd": repo_root,
        "yolo": false,
        "commit_id": "abc123",
        "operation_id": "omp-write-lineage",
        "tool_name": "write",
        "tool_input": { "path": "docs/a.md", "content": "hello" }
    })
    .to_string();

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "pre-tool-use"], &input);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for _ in 0..2 {
        let identity_request = rx.recv().expect("identity request should arrive");
        assert!(identity_request.starts_with("GET /v2/runtime/identity?"));
    }
    let body = request_json_body(&rx.recv().expect("authorize request should arrive"));
    assert_eq!(body["source"]["kind"], "hook");
    assert_eq!(body["source"]["event"], "omp_write_start");
    assert_eq!(body["agent"]["agent_id"], "omp-parent");
    assert_eq!(body["payload"]["action"], "write_file");
    assert_eq!(body["payload"]["targets"][0]["path"], "docs/a.md");
    assert!(body["payload"].get("commit_id").is_none());
    assert_eq!(body["agent"]["actor_id"], "main");
}

#[test]
fn omp_write_repo_external_target_prompts_for_scoped_approval() {
    let input = serde_json::json!({
        "runtime": "omp",
        "agent_id": "omp-parent",
        "cwd": "/repo",
        "yolo": false,
        "tool_name": "write",
        "tool_input": { "path": "/tmp/stateful-outside.txt", "content": "hello" }
    })
    .to_string();

    let outcome = handle_omp_pre_tool_use_with_runtime(
        &input,
        None,
        Some(Path::new("/repo")),
        Some(Path::new("/repo")),
    )
    .expect("omp pre-tool should parse");

    let OmpHookOutcome::Prompt { title, message } = outcome else {
        panic!("repo-external OMP write should prompt");
    };
    assert!(title.contains("external write"));
    assert!(message.contains("/tmp/stateful-outside.txt"));
    assert!(message.contains("stateful.autoApprove"));
}

#[test]
fn omp_edit_repo_external_target_prompts_for_scoped_approval() {
    let input = serde_json::json!({
        "runtime": "omp",
        "agent_id": "omp-parent",
        "cwd": "/repo",
        "yolo": false,
        "tool_name": "edit",
        "tool_input": { "input": "[/tmp/stateful-outside.txt#ABCD]\nSWAP 1.=1:\n+new\n" }
    })
    .to_string();

    let outcome = handle_omp_pre_tool_use_with_runtime(
        &input,
        None,
        Some(Path::new("/repo")),
        Some(Path::new("/repo")),
    )
    .expect("omp pre-tool should parse");

    let OmpHookOutcome::Prompt { title, message } = outcome else {
        panic!("repo-external OMP edit should prompt");
    };
    assert!(title.contains("external edit"));
    assert!(message.contains("/tmp/stateful-outside.txt"));
    assert!(message.contains("stateful.autoApprove"));
}

#[test]
fn omp_edit_move_to_repo_external_target_blocks_before_bypassing_repo_auth() {
    let input = serde_json::json!({
        "runtime": "omp",
        "agent_id": "omp-parent",
        "cwd": "/repo",
        "yolo": false,
        "tool_name": "edit",
        "tool_input": { "input": "[docs/a.md#ABCD]\nMV /tmp/stateful-outside.txt\n" }
    })
    .to_string();

    let outcome = handle_omp_pre_tool_use_with_runtime(
        &input,
        None,
        Some(Path::new("/repo")),
        Some(Path::new("/repo")),
    )
    .expect("omp pre-tool should parse");

    let OmpHookOutcome::Block { reason } = outcome else {
        panic!("repo-internal to repo-external OMP edit move should block");
    };
    assert!(reason.contains("split the operation"));
}

#[test]
fn omp_edit_mixed_repo_internal_and_external_targets_blocks() {
    let input = serde_json::json!({
        "runtime": "omp",
        "agent_id": "omp-parent",
        "cwd": "/repo",
        "yolo": false,
        "tool_name": "edit",
        "tool_input": {
            "input": "[docs/a.md#ABCD]\nSWAP 1.=1:\n+internal\n[/tmp/stateful-outside.txt#DCBA]\nSWAP 1.=1:\n+external\n"
        }
    })
    .to_string();

    let outcome = handle_omp_pre_tool_use_with_runtime(
        &input,
        None,
        Some(Path::new("/repo")),
        Some(Path::new("/repo")),
    )
    .expect("omp pre-tool should parse");

    let OmpHookOutcome::Block { reason } = outcome else {
        panic!("mixed repo-internal and repo-external OMP edit should block");
    };
    assert!(reason.contains("split the operation"));
}

#[test]
fn omp_unclassified_tools_are_manageable_with_stateful_tools_allowlist() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let yield_input = serde_json::json!({
        "agent_id": "omp-parent",
        "cwd": repo_root,
        "yolo": false,
        "tool_name": "yield",
        "tool_input": {"result": {"data": "done"}}
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "omp", "pre-tool-use"],
        &yield_input,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("OMP hook should print JSON");
    assert_eq!(stdout["decision"], "allow");
    let list = tool_list_for_repo(&paths, &repo_root).expect("tool list should load");
    assert!(list.unclassified_tools.is_empty());
    let lazy_write_resume_input = serde_json::json!({
        "agent_id": "omp-parent",
        "cwd": repo_root,
        "yolo": false,
        "tool_name": "lazy_write_resume",
        "tool_input": {"operation_id": "wait-123"}
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "omp", "pre-tool-use"],
        &lazy_write_resume_input,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("OMP hook should print JSON");
    assert_eq!(stdout["decision"], "allow");
    let list = tool_list_for_repo(&paths, &repo_root).expect("tool list should load");
    assert!(list.unclassified_tools.is_empty());

    let glob_input = serde_json::json!({
        "agent_id": "omp-parent",
        "cwd": repo_root,
        "yolo": false,
        "tool_name": "functions.glob",
        "tool_input": {"pattern": "**/*.rs"}
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "omp", "pre-tool-use"],
        &glob_input,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("OMP hook should print JSON");
    assert_eq!(stdout["decision"], "allow");
    let list = tool_list_for_repo(&paths, &repo_root).expect("tool list should load");
    assert!(list.unclassified_tools.is_empty());

    for (tool_name, tool_input) in [
        ("functions.goal", serde_json::json!({"op": "get"})),
        (
            "functions.hub",
            serde_json::json!({
                "op": "start",
                "name": "benchmark",
                "application": "stateful",
                "args": ["sandbox", "run"]
            }),
        ),
    ] {
        let input = serde_json::json!({
            "agent_id": "omp-parent",
            "cwd": repo_root,
            "yolo": false,
            "tool_name": tool_name,
            "tool_input": tool_input
        })
        .to_string();
        let output =
            run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "pre-tool-use"], &input);
        assert!(
            output.status.success(),
            "stateful hook failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("OMP hook should print JSON");
        assert_eq!(stdout["decision"], "allow", "{tool_name}");
    }
    let list = tool_list_for_repo(&paths, &repo_root).expect("tool list should load");
    assert!(list.unclassified_tools.is_empty());

    let input = serde_json::json!({
        "agent_id": "omp-parent",
        "cwd": repo_root,
        "yolo": false,
        "tool_name": "future_omp_widget",
        "tool_input": {}
    })
    .to_string();

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "pre-tool-use"], &input);
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("OMP hook should print JSON");
    assert_eq!(stdout["decision"], "block");
    assert!(
        stdout["reason"]
            .as_str()
            .expect("reason should be a string")
            .contains("unclassified OMP tool future_omp_widget")
    );
    let list = tool_list_for_repo(&paths, &repo_root).expect("tool list should load");
    assert_eq!(list.unclassified_tools, vec!["future_omp_widget"]);

    allow_tool_for_repo(&paths, &repo_root, "future_omp_widget")
        .expect("OMP tool should be user-allowed");
    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "pre-tool-use"], &input);
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("OMP hook should print JSON");
    assert_eq!(stdout["decision"], "allow");
}

#[test]
fn omp_glob_is_allowed_for_repo_with_stale_tool_allowlist() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    deny_tool_for_repo(&paths, &repo_root, "glob").expect("stale tool list should omit glob");
    let list = tool_list_for_repo(&paths, &repo_root).expect("tool list should load");
    assert!(!list.allowed_tools.iter().any(|tool| tool == "glob"));
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let glob_input = serde_json::json!({
        "agent_id": "omp-parent",
        "cwd": repo_root,
        "yolo": false,
        "tool_name": "functions.glob",
        "tool_input": {"pattern": "**/*.rs"}
    })
    .to_string();
    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "omp", "pre-tool-use"],
        &glob_input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("OMP hook should print JSON");
    assert_eq!(
        stdout["decision"], "allow",
        "glob should be intrinsically allowed, got {stdout}"
    );
    let list = tool_list_for_repo(&paths, &repo_root).expect("tool list should load");
    assert!(
        !list
            .unclassified_tools
            .iter()
            .any(|tool| tool == "glob" || tool == "functions.glob"),
        "glob should not be recorded as unclassified: {:?}",
        list.unclassified_tools
    );
}

#[test]
fn omp_parallel_tool_calls_is_allowed_for_repo_with_stale_tool_allowlist() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    deny_tool_for_repo(&paths, &repo_root, "parallel_tool_calls")
        .expect("stale tool list should omit parallel_tool_calls");
    let list = tool_list_for_repo(&paths, &repo_root).expect("tool list should load");
    assert!(
        !list
            .allowed_tools
            .iter()
            .any(|tool| tool == "parallel_tool_calls")
    );
    let (runtime, _rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "omp-parent",
        "cwd": repo_root,
        "yolo": false,
        "tool_name": "parallel_tool_calls",
        "tool_input": {"tool_uses": []}
    })
    .to_string();
    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "pre-tool-use"], &input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("OMP hook should print JSON");
    assert_eq!(
        stdout["decision"], "allow",
        "parallel_tool_calls should be intrinsically allowed, got {stdout}"
    );
    let list = tool_list_for_repo(&paths, &repo_root).expect("tool list should load");
    assert!(
        !list
            .unclassified_tools
            .iter()
            .any(|tool| tool == "parallel_tool_calls"),
        "parallel_tool_calls should not be recorded as unclassified: {:?}",
        list.unclassified_tools
    );
}

#[test]
fn run_hook_omp_pre_tool_use_prints_extension_decision() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("docs")).expect("repo docs should create");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"decision":"allow","message":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": repo_root,
        "yolo": false,
        "tool_name": "write",
        "tool_input": { "path": "docs/a.md", "content": "hello" }
    })
    .to_string();

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "pre-tool-use"], &input);
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("OMP hook should print JSON");
    assert_eq!(stdout["decision"], "allow");
    let body = request_json_body(&rx.recv().expect("authorize request should arrive"));
    assert_eq!(body["agent"]["agent_id"], "omp-parent");
}

#[test]
fn run_hook_omp_env_runtime_derives_workspace_id_from_enabled_repo() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("docs")).expect("repo docs should create");
    enable_test_repo(&paths, &repo_root);
    let expected_workspace_id =
        workspace_id_for_enabled_repo(&paths, &repo_root).expect("repo workspace id should load");
    let expected_root = repo_root
        .canonicalize()
        .expect("repo root should canonicalize");
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"status":"ok"}"#,
        r#"{"decision":"allow","message":"ok"}"#,
    ]);
    let extra_env = [
        ("STATEFUL_SERVER_URL", runtime.base_url.as_str()),
        ("STATEFUL_SERVER_TOKEN", runtime.token.as_str()),
    ];

    let session_start = serde_json::json!({
        "agent_id": "omp-parent",
        "cwd": repo_root,
        "omp_agent_id": "main"
    })
    .to_string();
    let output = run_hook_subprocess_with_extra_env(
        &repo_root,
        &paths,
        &["hook", "omp", "session-start"],
        &session_start,
        &extra_env,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout.clone()).expect("stdout should be utf8");
    let session_start_output: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("session start stdout should be json");
    assert_eq!(session_start_output["decision"], "allow");
    assert_eq!(session_start_output["workspace_id"], expected_workspace_id);
    assert_eq!(
        session_start_output["notifications_stream"]["agent_id"],
        "omp-parent"
    );
    assert_eq!(
        session_start_output["notifications_stream"]["workspace_id"],
        expected_workspace_id
    );
    let register = request_json_body(&rx.recv().expect("session register should arrive"));
    assert_eq!(register["agent"]["agent_id"], "omp-parent");
    assert_eq!(register["workspace"]["workspace_id"], expected_workspace_id);
    assert_ne!(register["workspace"]["workspace_id"], "unknown");

    let pre_tool = serde_json::json!({
        "agent_id": "omp-parent",
        "cwd": repo_root,
        "yolo": false,
        "omp_agent_id": "main",
        "tool_name": "write",
        "tool_input": { "path": "docs/a.md", "content": "hello" }
    })
    .to_string();
    let output = run_hook_subprocess_with_extra_env(
        &repo_root,
        &paths,
        &["hook", "omp", "pre-tool-use"],
        &pre_tool,
        &extra_env,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("OMP hook should print JSON");
    assert_eq!(stdout["decision"], "allow");
    let authorize = request_json_body(&rx.recv().expect("authorize request should arrive"));
    assert_eq!(authorize["agent"]["agent_id"], "omp-parent");
    assert_eq!(
        authorize["workspace"]["workspace_id"],
        expected_workspace_id
    );
    assert_eq!(
        authorize["workspace"]["root"],
        expected_root.to_string_lossy().as_ref()
    );
    assert_ne!(authorize["workspace"]["workspace_id"], "unknown");
}

#[test]
fn omp_edit_extracts_hashline_file_targets() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(repo_root.join("docs")).expect("repo docs should create");
    fs::write(repo_root.join("docs/a.md"), "before\n").expect("fixture should write");
    enable_test_repo(&paths, &repo_root);
    let identity = r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#;
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        identity,
        identity,
        r#"{"intent_id":"intent-1","decision":{"decision":"deny","reason_code":"active_claim_conflict","message":"blocked"}}"#,
        r#"{"status":"completed"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("runtime file should write");
    let input = serde_json::json!({
        "agent_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": repo_root,
        "yolo": false,
        "operation_id": "omp-edit-hashline",
        "tool_name": "edit",
        "tool_input": { "input": "[docs/a.md#ABCD]\nSWAP 1.=1:\n+new\n" }
    })
    .to_string();

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "pre-tool-use"], &input);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for _ in 0..2 {
        let identity_request = rx.recv().expect("identity request should arrive");
        assert!(identity_request.starts_with("GET /v2/runtime/identity?"));
    }
    let body = request_json_body(&rx.recv().expect("authorize request should arrive"));
    assert_eq!(body["payload"]["action"], "write_file");
    assert_eq!(body["payload"]["targets"][0]["path"], "docs/a.md");
}

#[test]
fn pre_tool_use_rejects_codex_mixed_patch_actions_before_authorization() {
    let input = serde_json::json!({
        "agent_id": "codex-agent",
        "tool_name": "apply_patch",
        "tool_use_id": "mixed-actions",
        "tool_input": {
            "patch": "*** Begin Patch\n*** Update File: docs/a.md\n@@\n-old\n+new\n*** Delete File: docs/b.md\n*** End Patch"
        }
    })
    .to_string();

    let HookOutcome::Deny { reason } =
        handle_pre_tool_use(&input).expect("mixed patch should parse")
    else {
        panic!("mixed patch must be denied");
    };
    assert!(reason.contains("mixes write actions"), "{reason}");
}

#[test]
fn omp_edit_rejects_mixed_patch_actions_before_authorization() {
    let runtime = ServerRuntime::new("http://127.0.0.1:9", "token", "w1", 1);
    let input = serde_json::json!({
        "agent_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": "/repo",
        "operation_id": "omp-edit-mixed-actions",
        "tool_name": "edit",
        "tool_input": {
            "input": "[docs/a.md#ABCD]\nSWAP 1.=1:\n+write\n[docs/b.md#DCBA]\nMV docs/c.md\n"
        }
    })
    .to_string();

    let outcome = handle_omp_pre_tool_use_with_runtime(
        &input,
        Some(&runtime),
        Some(Path::new("/repo")),
        Some(Path::new("/repo")),
    )
    .expect("mixed OMP edit should parse");

    let OmpHookOutcome::Block { reason } = outcome else {
        panic!("mixed OMP edit should block before authorization");
    };
    assert!(reason.contains("split the operation"), "{reason}");
}

#[test]
fn omp_write_uses_tool_input_reservation_when_top_level_reservation_is_blank() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(repo_root.join("docs")).expect("repo docs should create");
    fs::write(repo_root.join("docs/a.md"), "before\n").expect("fixture should write");
    enable_test_repo(&paths, &repo_root);
    let identity = r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#;
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        identity,
        identity,
        r#"{"intent_id":"intent-1","decision":{"decision":"deny","reason_code":"missing_reservation","message":"missing"}}"#,
        r#"{"status":"completed"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("runtime file should write");
    let input = serde_json::json!({
        "agent_id": "omp-parent",
        "reservation_id": "   ",
        "workspace_id": runtime.workspace_id,
        "cwd": repo_root,
        "yolo": false,
        "operation_id": "omp-write-explicit-reservation",
        "tool_name": "write",
        "tool_input": {
            "reservation_id": "explicit-reservation",
            "path": "docs/a.md",
            "content": "hello"
        }
    })
    .to_string();

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "pre-tool-use"], &input);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for _ in 0..2 {
        let identity_request = rx.recv().expect("identity request should arrive");
        assert!(identity_request.starts_with("GET /v2/runtime/identity?"));
    }
    let authorize = request_json_body(&rx.recv().expect("authorize should arrive"));
    assert_eq!(
        authorize["payload"]["reservation_id"],
        "explicit-reservation"
    );
}

#[test]
fn omp_write_releases_auto_claim_when_retry_authorization_blocks() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(repo_root.join("docs")).expect("repo docs should create");
    fs::write(repo_root.join("docs/a.md"), "before\n").expect("fixture should write");
    enable_test_repo(&paths, &repo_root);
    let identity = r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#;
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        identity,
        identity,
        r#"{"decision":"deny","message":"Supported writes require active file or directory reservation.","reason_code":"missing_reservation"}"#,
        r#"{"status":"ok","reservation_id":"auto-reservation"}"#,
        r#"{"claims":[{"claim_id":"auto-claim"}]}"#,
        r#"{"decision":"deny","message":"Write target is covered by another active agent claim.","reason_code":"active_claim_conflict"}"#,
        r#"{"status":"ok"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("runtime file should write");
    let input = serde_json::json!({
        "agent_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": repo_root,
        "yolo": false,
        "operation_id": "omp-write-release-claim",
        "tool_name": "write",
        "tool_input": { "path": "docs/a.md", "content": "hello" }
    })
    .to_string();

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "pre-tool-use"], &input);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for _ in 0..2 {
        let identity_request = rx.recv().expect("identity request should arrive");
        assert!(identity_request.starts_with("GET /v2/runtime/identity?"));
    }
    let _first_authorize = rx.recv().expect("first authorize should arrive");
    let _declare = rx.recv().expect("reservation declare should arrive");
    let _claim = rx.recv().expect("claim acquire should arrive");
    let _retry_authorize = rx.recv().expect("retry authorize should arrive");
    let release = request_json_body(
        &rx.recv_timeout(Duration::from_secs(1))
            .expect("claim release should arrive"),
    );
    assert_eq!(release["agent"]["agent_id"], "omp-parent");
    assert_eq!(release["workspace"]["workspace_id"], runtime.workspace_id);
    assert_eq!(release["payload"]["claim_id"], "auto-claim");
}

#[test]
fn omp_raw_bash_authorizes_trusted_write_target_sandbox_run() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let paths = GlobalPaths::new(temp.path().join("home"));
    let repo_root = temp.path().join("repo");
    fs::create_dir_all(repo_root.join("docs")).expect("repo docs should create");
    fs::write(repo_root.join("docs/a.md"), "before\n").expect("fixture should write");
    enable_test_repo(&paths, &repo_root);
    let stateful = env!("CARGO_BIN_EXE_stateful");
    let identity = r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#;
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        identity,
        identity,
        r#"{"intent_id":"intent-1","decision":{"decision":"deny","reason_code":"active_claim_conflict","message":"blocked"}}"#,
        r#"{"status":"completed"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("runtime file should write");
    let input = serde_json::json!({
        "agent_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": repo_root,
        "yolo": false,
        "operation_id": "omp-bash-write",
        "tool_name": "bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs write-targets --write-target docs/a.md --command 'printf ok > docs/a.md'")
        }
    })
    .to_string();
    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "omp", "pre-tool-use"], &input);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for _ in 0..2 {
        assert!(
            rx.recv()
                .expect("identity request should arrive")
                .starts_with("GET /v2/runtime/identity?")
        );
    }
    let body = request_json_body(&rx.recv().expect("authorize request should arrive"));
    assert_eq!(body["payload"]["action"], "write_file");
    assert_eq!(body["payload"]["targets"][0]["path"], "docs/a.md");
}

#[test]
fn omp_removed_generated_command_tools_are_not_allowlisted() {
    for tool_name in [
        "sandbox_bash",
        "ext_ro_bash",
        "ext_rw_bash",
        "process_find",
        "sandbox_job_poll",
    ] {
        let input = serde_json::json!({
            "agent_id": "omp-parent",
            "cwd": "/repo",
            "yolo": false,
            "tool_name": tool_name,
            "tool_input": {
                "command": "pwd",
                "purpose": "test removed generated tool",
                "fs": "read-only"
            }
        })
        .to_string();

        let OmpHookOutcome::Block { reason } = handle_omp_pre_tool_use_with_runtime(
            &input,
            None,
            Some(Path::new("/repo")),
            Some(Path::new("/repo")),
        )
        .expect("removed generated command tool should be classified") else {
            panic!("{tool_name} should no longer be allowlisted");
        };
        assert!(reason.contains("not classified") || reason.contains("unclassified"));
    }
}

#[test]
fn omp_raw_bash_allows_trusted_external_sandbox_run_for_extension_preflight() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "omp-parent",
        "cwd": "/repo",
        "yolo": false,
        "tool_name": "bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs external --purpose 'write external artifact' --write-target /tmp/stateful-outside.txt --command 'printf ok > /tmp/stateful-outside.txt'")
        }
    })
    .to_string();

    assert_eq!(
        handle_omp_pre_tool_use_with_runtime(
            &input,
            None,
            Some(Path::new("/repo")),
            Some(Path::new("/repo"))
        )
        .expect("trusted external sandbox run should authorize"),
        OmpHookOutcome::Allow
    );
}

#[test]
fn omp_repo_internal_raw_bash_rejects_shell_writes_and_unsafe_find_actions() {
    for command in ["ls > docs/a.md", "find . -delete"] {
        let input = serde_json::json!({
            "agent_id": "omp-parent",
            "cwd": "/repo",
            "yolo": false,
            "tool_name": "bash",
            "tool_input": { "command": command }
        })
        .to_string();
        assert!(matches!(
            handle_omp_pre_tool_use_with_runtime(
                &input,
                None,
                Some(Path::new("/repo")),
                Some(Path::new("/repo"))
            )
            .expect("unsafe repo-internal raw bash should be classified"),
            OmpHookOutcome::Block { .. }
        ));
    }
}

#[test]
fn omp_repo_internal_raw_bash_allows_only_trusted_sandbox_requests() {
    let stateful = trusted_stateful_path();
    let allowed =
        format!("{stateful} sandbox run --fs read-only --network disabled --command 'pwd'");
    let denied = ["pwd".to_string(), "python scripts/gen.py".to_string()];

    let input = serde_json::json!({
        "agent_id": "omp-parent",
        "cwd": "/repo",
        "yolo": false,
        "tool_name": "bash",
        "tool_input": { "command": allowed }
    })
    .to_string();
    assert_eq!(
        handle_omp_pre_tool_use_with_runtime(
            &input,
            None,
            Some(Path::new("/repo")),
            Some(Path::new("/repo"))
        )
        .expect("trusted read-only sandbox run should authorize"),
        OmpHookOutcome::Allow
    );

    for command in denied {
        let input = serde_json::json!({
            "agent_id": "omp-parent",
            "cwd": "/repo",
            "yolo": false,
            "tool_name": "bash",
            "tool_input": { "command": command }
        })
        .to_string();

        let OmpHookOutcome::Block { reason } = handle_omp_pre_tool_use_with_runtime(
            &input,
            None,
            Some(Path::new("/repo")),
            Some(Path::new("/repo")),
        )
        .expect("repo-internal raw bash should be classified") else {
            panic!("non-stateful raw OMP bash should be denied");
        };
        assert!(reason.contains("OMP raw bash is denied"));
        assert!(reason.contains("stateful sandbox run"));
    }
}

#[test]
fn omp_namespaced_bash_allows_only_trusted_sandbox_requests() {
    let stateful = trusted_stateful_path();
    for (command, allowed) in [
        ("pwd".to_string(), false),
        (
            format!("{stateful} sandbox run --fs read-only --network disabled --command 'pwd'"),
            true,
        ),
    ] {
        let input = serde_json::json!({
            "agent_id": "omp-parent",
            "cwd": "/repo",
            "yolo": false,
            "tool_name": "functions.bash",
            "tool_input": { "command": command }
        })
        .to_string();
        let outcome = handle_omp_pre_tool_use_with_runtime(
            &input,
            None,
            Some(Path::new("/repo")),
            Some(Path::new("/repo")),
        )
        .expect("namespaced bash command should be classified");
        if allowed {
            assert_eq!(outcome, OmpHookOutcome::Allow);
        } else {
            let OmpHookOutcome::Block { reason } = outcome else {
                panic!("non-stateful namespaced raw OMP bash should be denied");
            };
            assert!(reason.contains("OMP raw functions.bash is denied"));
            assert!(reason.contains("stateful sandbox run"));
        }
    }
}

#[test]
fn omp_eval_tools_are_denied_even_for_sandbox_run_requests() {
    for tool_name in [
        "eval",
        "py",
        "python",
        "javascript",
        "js",
        "rb",
        "ruby",
        "jl",
        "julia",
    ] {
        let input = serde_json::json!({
            "agent_id": "omp-parent",
            "cwd": "/repo",
            "yolo": false,
            "tool_name": tool_name,
            "tool_input": { "code": "print('hello')" }
        })
        .to_string();
        let OmpHookOutcome::Block { reason } = handle_omp_pre_tool_use_with_runtime(
            &input,
            None,
            Some(Path::new("/repo")),
            Some(Path::new("/repo")),
        )
        .expect("raw eval tool should be classified") else {
            panic!("raw OMP eval tool should block");
        };
        assert!(reason.contains(&format!("OMP eval tool {tool_name} is denied")));
        assert!(reason.contains("built-in Bash"));
        assert!(reason.contains("stateful sandbox run"));
        assert!(reason.contains("stateful sandbox process find"));
        assert!(!reason.contains("sandbox_bash"));
        assert!(!reason.contains("process_find"));
        assert!(!reason.contains("ext_ro_bash"));
        assert!(!reason.contains("ext_rw_bash"));
    }

    let raw_python_input = serde_json::json!({
        "agent_id": "omp-parent",
        "cwd": "/repo",
        "yolo": false,
        "tool_name": "functions.python",
        "tool_input": { "code": "print('hello')" }
    })
    .to_string();
    let OmpHookOutcome::Block { reason } = handle_omp_pre_tool_use_with_runtime(
        &raw_python_input,
        None,
        Some(Path::new("/repo")),
        Some(Path::new("/repo")),
    )
    .expect("functions.python eval tool should be classified") else {
        panic!("raw OMP python should block");
    };
    assert!(reason.contains("OMP eval tool functions.python is denied"));
    assert!(reason.contains("built-in Bash"));
    assert!(reason.contains("stateful sandbox run"));
    assert!(reason.contains("stateful sandbox process find"));
    assert!(!reason.contains("sandbox_bash"));
    assert!(!reason.contains("process_find"));
    assert!(!reason.contains("ext_ro_bash"));
    assert!(!reason.contains("ext_rw_bash"));

    let stateful = trusted_stateful_path();
    let sandboxed_python_input = serde_json::json!({
        "agent_id": "omp-parent",
        "cwd": "/repo",
        "yolo": false,
        "tool_name": "python",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs read-only --network disabled --command 'python -c \"print(1)\"'")
        }
    })
    .to_string();
    let OmpHookOutcome::Block { reason } = handle_omp_pre_tool_use_with_runtime(
        &sandboxed_python_input,
        None,
        Some(Path::new("/repo")),
        Some(Path::new("/repo")),
    )
    .expect("sandbox-run python eval tool should be classified") else {
        panic!("sandbox-run through raw OMP python should still block");
    };
    assert!(reason.contains("OMP eval tool python is denied"));
    assert!(reason.contains("built-in Bash"));
    assert!(reason.contains("stateful sandbox run"));
    assert!(reason.contains("stateful sandbox process find"));
    assert!(!reason.contains("sandbox_bash"));
    assert!(!reason.contains("process_find"));
    assert!(!reason.contains("ext_ro_bash"));
    assert!(!reason.contains("ext_rw_bash"));

    let repo_external_python_input = serde_json::json!({
        "agent_id": "omp-parent",
        "cwd": "/tmp/outside",
        "yolo": false,
        "tool_name": "python",
        "tool_input": { "code": "print('outside')" }
    })
    .to_string();
    let OmpHookOutcome::Block { reason } = handle_omp_pre_tool_use_with_runtime(
        &repo_external_python_input,
        None,
        Some(Path::new("/repo")),
        Some(Path::new("/tmp/outside")),
    )
    .expect("repo-external python eval tool should be classified") else {
        panic!("repo-external OMP python should block");
    };
    assert!(reason.contains("OMP eval tool python is denied"));
    assert!(reason.contains("built-in Bash"));
    assert!(reason.contains("stateful sandbox run"));
    assert!(reason.contains("stateful sandbox process find"));
    assert!(!reason.contains("sandbox_bash"));
    assert!(!reason.contains("process_find"));
    assert!(!reason.contains("ext_ro_bash"));
    assert!(!reason.contains("ext_rw_bash"));
}

#[test]
fn omp_allows_classified_read_only_and_non_file_writing_tools() {
    for tool_name in [
        "ask",
        "ast_grep",
        "browser",
        "find",
        "generate_image",
        "grep",
        "irc",
        "job",
        "read",
        "report_tool_issue",
        "search",
        "search_tool_bm25",
        "task",
        "todo",
        "web_search",
        "mcp__stateful_state_current_read",
        "mcp__stateful_state_reservation_declare",
        "state_current_read",
        "state_reservation_declare",
    ] {
        let input = serde_json::json!({
            "agent_id": "omp-parent",
            "cwd": "/repo",
            "yolo": false,
            "tool_name": tool_name,
            "tool_input": { "path": "README.md" }
        })
        .to_string();

        assert_eq!(
            handle_omp_pre_tool_use_with_runtime(
                &input,
                None,
                Some(Path::new("/repo")),
                Some(Path::new("/repo"))
            )
            .expect("classified read-only tool should authorize"),
            OmpHookOutcome::Allow
        );
    }
}

#[test]
fn omp_repo_external_raw_bash_blocks_without_external_sandbox_profile() {
    let input = serde_json::json!({
        "agent_id": "omp-parent",
        "cwd": "/tmp/outside",
        "yolo": false,
        "tool_name": "bash",
        "tool_input": { "command": "python /tmp/tool.py" }
    })
    .to_string();

    let outcome = handle_omp_pre_tool_use_with_runtime(
        &input,
        None,
        Some(Path::new("/repo")),
        Some(Path::new("/tmp/outside")),
    )
    .expect("repo-external raw bash should be classified");
    let OmpHookOutcome::Block { reason } = outcome else {
        panic!("repo-external targetless bash should block");
    };
    assert!(reason.contains("OMP raw bash is denied"));
    assert!(reason.contains("stateful sandbox run"));
}

#[test]
fn omp_allows_sandbox_external_profile_after_extension_preflight() {
    let stateful = trusted_stateful_path();
    let input = serde_json::json!({
        "agent_id": "omp-parent",
        "cwd": "/repo",
        "yolo": false,
        "tool_name": "bash",
        "tool_input": {
            "command": format!("{stateful} sandbox run --fs external --purpose 'write external artifact' --write-target /tmp/stateful-outside.txt --command 'printf ok > /tmp/stateful-outside.txt'")
        }
    })
    .to_string();

    assert_eq!(
        handle_omp_pre_tool_use_with_runtime(
            &input,
            None,
            Some(Path::new("/repo")),
            Some(Path::new("/repo")),
        )
        .expect("external sandbox profile should authorize"),
        OmpHookOutcome::Allow
    );
}

#[test]
fn omp_yolo_does_not_downgrade_server_denial() {
    let (runtime, _rx) =
        spawn_fake_stateful_server(r#"{"decision":"deny","message":"missing claim"}"#);
    let input = serde_json::json!({
        "agent_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": "/repo",
        "yolo": true,
        "tool_name": "write",
        "tool_input": { "path": "docs/a.md", "content": "hello" }
    })
    .to_string();

    assert!(matches!(
        handle_omp_pre_tool_use_with_runtime(
            &input,
            Some(&runtime),
            Some(Path::new("/repo")),
            Some(Path::new("/repo"))
        )
        .expect("yolo write denial should be classified"),
        OmpHookOutcome::Block { .. }
    ));
}

#[test]
fn omp_session_start_posts_v2_presence_registration() {
    let identity = r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#;
    let (runtime, rx) =
        spawn_fake_stateful_server_sequence(vec![identity, r#"{"agent_id":"omp-parent"}"#]);
    let input = serde_json::json!({
        "agent_id": "omp-parent",
        "workspace_id": runtime.workspace_id,
        "cwd": "/repo",
        "omp_agent_id": "main",
        "commit_id": "abc123"
    })
    .to_string();

    let output = handle_omp_session_start_with_runtime(&input, &runtime)
        .expect("omp session start should post register");
    assert_eq!(output.decision, "allow");
    assert_eq!(output.agent_id, "omp-parent");
    assert_eq!(output.workspace_id, runtime.workspace_id);
    assert_eq!(output.notifications_stream.base_url, runtime.base_url);
    assert_eq!(
        output.notifications_stream.authorization,
        format!("Bearer {}", runtime.token)
    );

    assert!(
        rx.recv()
            .expect("registration identity request should arrive")
            .contains("GET /v2/runtime/identity?")
    );
    let request = rx.recv().expect("session register request should arrive");
    assert!(request.contains("POST /v2/session/register HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v2");
    assert_eq!(body["agent"]["agent_id"], "omp-parent");
    assert_eq!(body["agent"]["actor_id"], "main");
    assert_eq!(body["workspace"]["workspace_id"], runtime.workspace_id);
    assert_eq!(body["source"]["event"], "omp_session_start");
    assert_eq!(body["payload"]["first_prompt"], serde_json::Value::Null);
}

#[test]
fn omp_subagent_post_tool_uses_child_session_and_parent_metadata() {
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    let input = serde_json::json!({
        "agent_id": "omp-child",
        "parent_agent_id": "omp-parent",
        "omp_agent_id": "WorkerA",
        "workspace_id": runtime.workspace_id,
        "cwd": "/repo",
        "tool_name": "write",
        "tool_input": { "path": "docs/a.md", "content": "hello" }
    })
    .to_string();

    handle_omp_post_tool_use_with_runtime(&input, &runtime)
        .expect("omp post tool should post heartbeat");

    let body = request_json_body(&rx.recv().expect("heartbeat request should arrive"));
    assert_eq!(body["agent"]["agent_id"], "omp-child");
    assert_eq!(body["workspace"]["workspace_id"], runtime.workspace_id);
    assert_eq!(body["agent"]["parent_agent_id"], "omp-parent");
    assert_eq!(body["agent"]["actor_id"], "WorkerA");
}

#[test]
fn pre_tool_use_edit_posts_authorize_and_denies_when_server_denies() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    fs::write(repo_root.join("src/auth.ts"), b"old contents\n")
        .expect("observed file should be writable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Write target is outside active reservation scope.","required_next_action":"Declare matching reservation."}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/auth.ts",
            "old_string": "old",
            "new_string": "new"
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v2/authorize HTTP/1.1"));
    assert!(request.contains("\"action\":\"write_file\""));
    assert!(request.contains("\"path\":\"src/auth.ts\""));
    let body = request_json_body(&request);
    let targets = body["payload"]["targets"]
        .as_array()
        .expect("targets should be present");
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0]["path"], "src/auth.ts");
    assert_eq!(targets[0]["before"]["exists"], true);
    assert_eq!(targets[0]["before"]["byte_len"], 13);
    assert!(targets[0]["before"]["sha256"].is_string());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");
}

#[test]
fn pre_tool_use_edit_allows_with_context_when_server_warns() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    fs::write(repo_root.join("src/auth.ts"), b"old contents\n")
        .expect("observed file should be writable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"warn","reason_code":"missing_reservation","message":"Review context first.","required_next_action":"Reread before writing."}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/auth.ts",
            "old_string": "old",
            "new_string": "new"
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _request = rx.recv().expect("captured request should arrive");
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("warn outcome should serialize");
    assert_eq!(
        json["hookSpecificOutput"]["additionalContext"],
        "stateful warning: Review context first."
    );
    assert!(
        json["hookSpecificOutput"]
            .get("permissionDecision")
            .is_none()
    );
}

#[test]
fn pre_tool_use_edit_denies_when_authorize_connection_drops() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server_dropping_authorize();
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/auth.ts",
            "old_string": "old",
            "new_string": "new"
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook should return a structured denial, not crash: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v2/authorize HTTP/1.1"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"permissionDecision\":\"deny\""));
    assert!(stdout.contains("stateful.v2 write authorization failed"));
}

#[test]
fn pre_tool_use_edit_posts_authorize_without_rendering_live_context_when_server_allows() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    fs::write(repo_root.join("src/auth.ts"), b"old contents\n")
        .expect("observed file should be writable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        r#"{"status":"ok","prompt_text":"Nearby Activity\n- unexpected context"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/auth.ts",
            "old_string": "old",
            "new_string": "new"
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v2/authorize HTTP/1.1"));
    assert!(request.contains("\"action\":\"write_file\""));
    assert!(request.contains("\"path\":\"src/auth.ts\""));
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "Edit authorization should not render live context"
    );
    assert!(
        output.stdout.is_empty(),
        "allowed Edit writes should not inject live context: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn pre_tool_use_edit_relative_path_is_resolved_from_payload_cwd() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    let docs_dir = repo_root.join("docs");
    fs::create_dir_all(&docs_dir).expect("docs dir should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": docs_dir,
        "hook_event_name": "PreToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "plan.md",
            "old_string": "old",
            "new_string": "new"
        }
    })
    .to_string();

    let output = run_hook_subprocess_from(
        temp_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("\"action\":\"write_file\""));
    assert!(request.contains("\"path\":\"docs/plan.md\""));
}

#[test]
fn run_hook_uses_payload_cwd_for_repo_gate() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    let outside = temp_root.join("outside");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    fs::create_dir_all(&outside).expect("outside dir should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "s-cwd",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "tool_input": {
            "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
        }
    })
    .to_string();

    let output =
        run_hook_subprocess_from(&outside, &paths, &["hook", "codex", "pre-tool-use"], &input);

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v2/authorize HTTP/1.1"));
    assert!(request.contains("\"agent_id\":\"s-cwd\""));
    assert_current_agent_context_absent(&repo_root);
    assert!(!outside.join(".stateful_core/runtime/session.json").exists());
}

#[test]
fn pre_tool_use_apply_patch_posts_authorize_and_allows_when_server_allows() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        r#"{"status":"ok","prompt_text":"Nearby Activity\n- unexpected context"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v2/authorize HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v2");
    assert_eq!(body["agent"]["agent_id"], "s1");
    assert_eq!(
        body["workspace"]["workspace_id"],
        workspace_id_for_enabled_repo(&paths, &repo_root).expect("repo workspace id should load")
    );
    assert!(
        body["workspace"]["repo_id"]
            .as_str()
            .expect("repo_id should be a string")
            .starts_with("repo-")
    );
    assert_eq!(
        body["workspace"]["worktree_id"],
        body["workspace"]["repo_id"]
    );
    assert_eq!(
        body["workspace"]["root"],
        repo_root
            .canonicalize()
            .expect("repo root should canonicalize")
            .to_string_lossy()
            .as_ref()
    );
    assert!(
        !body["workspace"]["branch"]
            .as_str()
            .expect("branch should be a string")
            .is_empty()
    );
    assert_eq!(body["source"]["kind"], "hook");
    assert_eq!(body["source"]["event"], "codex_write_start");
    assert_eq!(body["payload"]["action"], "write_file");
    assert_eq!(body["payload"]["targets"][0]["path"], "src/auth.ts");
    assert_eq!(body["payload"]["reservation_id"], serde_json::Value::Null);
    assert!(body.get("action").is_none());
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "apply_patch authorization should not render live context"
    );
    assert!(
        output.stdout.is_empty(),
        "allowed writes should not inject live context: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn pre_tool_use_apply_patch_invalid_agent_id_does_not_write_denial_marker() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"decision":"deny","reason_code":"active_claim_conflict","message":"Write target is covered by another active agent claim.","required_next_action":"Reread target, then claim the reservation before writing."}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "agent_id": "../escape",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("hook stdout should be JSON");
    assert_eq!(stdout["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "unsupported session id should fail closed before authorization"
    );
    assert!(!repo_root.join(".stateful_core/runtime/escape").exists());
}

#[test]
fn pre_tool_use_apply_patch_denial_does_not_render_live_context() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server_sequence(vec![
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Write target is outside active reservation scope.","required_next_action":"Declare matching reservation."}"#,
        r#"{"status":"ok","prompt_text":"Nearby Activity\n- unexpected context"}"#,
    ]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _authorize_request = rx.recv().expect("authorize request should arrive");
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "apply_patch denial should not render live context"
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");
    let reason = json["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .expect("deny reason should be text");
    assert!(reason.contains("Declare matching reservation."));
    assert!(!reason.contains("Nearby Activity"));
}

#[test]
fn pre_tool_use_apply_patch_sends_base_observation_for_existing_file() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(repo_root.join("src")).expect("repo src should be creatable");
    fs::write(repo_root.join("src/auth.ts"), b"original contents\n")
        .expect("observed file should be writable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "tool_input": {
            "command": "*** Begin Patch\n*** Update File: src/auth.ts\n@@\n-original contents\n+updated contents\n*** End Patch\n"
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    let body = request_json_body(&request);
    let targets = body["payload"]["targets"]
        .as_array()
        .expect("targets should be present");
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0]["path"], "src/auth.ts");
    assert_eq!(targets[0]["before"]["exists"], true);
    assert_eq!(targets[0]["before"]["byte_len"], 18);
    assert!(targets[0]["before"]["sha256"].is_string());
}

#[test]
fn pre_tool_use_apply_patch_patch_field_authorizes_every_file_target() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "tool_input": {
            "patch": "*** Begin Patch\n*** Update File: doc.txt\n@@\n base\n*** Update File: persisted_doc.txt\n@@\n base\n*** End Patch\n"
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("authorize request should arrive");
    let body = request_json_body(&request);
    let targets = body["payload"]["targets"]
        .as_array()
        .expect("targets should be present")
        .iter()
        .map(|target| target["path"].as_str().expect("target path should be text"))
        .collect::<Vec<_>>();
    assert_eq!(targets, ["doc.txt", "persisted_doc.txt"]);
    assert!(output.stdout.is_empty());
}

#[test]
fn pre_tool_use_apply_patch_move_authorizes_source_and_destination() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Move target is outside task reservation exact scope.","required_next_action":"Add exact source and destination scopes to the task reservation and acquire both claims."}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "tool_input": {
            "patch": "*** Begin Patch\n*** Update File: old.txt\n*** Move to: new.txt\n@@\n-old\n+new\n*** End Patch\n"
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("move authorize request should arrive");
    let body = request_json_body(&request);
    assert_eq!(body["payload"]["action"], "move_file");
    let targets = body["payload"]["targets"]
        .as_array()
        .expect("targets should be present");
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0]["path"], "old.txt");
    assert_eq!(targets[1]["path"], "new.txt");
    assert_eq!(targets[0]["before"]["exists"], false);
    assert_eq!(targets[1]["before"]["exists"], false);
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        json["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .expect("denial reason should be text")
            .contains("Add exact source and destination scopes")
    );
}

#[test]
fn pre_tool_use_apply_patch_raw_string_payload_posts_authorize() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "tool_input": "*** Begin Patch\n*** Update File: doc.txt\n@@\n base\n*** End Patch\n"
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("authorize request should arrive");
    assert!(request.contains("\"action\":\"write_file\""));
    assert!(request.contains("\"path\":\"doc.txt\""));
    assert!(request.contains("\"event\":\"codex_write_start\""));
}

#[test]
fn pre_tool_use_file_change_posts_authorize_for_changed_paths() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "file_change",
        "tool_use_id": "file-change-write",
        "tool_input": {
            "changes": [
                {"path": "doc.txt", "kind": "update"}
            ]
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("authorize request should arrive");
    assert!(request.contains("\"action\":\"write_file\""));
    assert!(request.contains("\"path\":\"doc.txt\""));
}

#[test]
fn hook_pre_tool_use_discovers_global_runtime_file() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    let paths = GlobalPaths::new(temp_root.join("home"));
    enable_test_repo(&paths, &repo_root);
    let runtime_pid = Arc::new(AtomicU32::new(0));
    let (mut runtime, rx) = spawn_fake_stateful_server_with_runtime_pid(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        Arc::clone(&runtime_pid),
    );

    let input = r#"{
      "agent_id": "s-global",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_use_id": "runtime-discovery-write",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let mut child = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["hook", "codex", "pre-tool-use"])
        .current_dir(&repo_root)
        .env_clear()
        .env("STATEFUL_HOME", &paths.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("stateful hook should spawn");
    runtime.pid = child.id();
    runtime_pid.store(runtime.pid, Ordering::Release);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");
    let mut stdin = child.stdin.take().expect("stdin should be piped");
    stdin
        .write_all(input.as_bytes())
        .expect("hook input should write");
    drop(stdin);
    let output = child
        .wait_with_output()
        .expect("stateful hook should complete");

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v2/authorize HTTP/1.1"));
    assert_eq!(request_json_body(&request)["workspace"]["workspace_id"], "w1");
}

#[test]
fn pre_tool_use_apply_patch_denies_when_server_denies() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"deny","reason_code":"scope_mismatch","message":"Write target is outside active reservation scope.","required_next_action":"Declare matching reservation."}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_use_id": "deny-authorization",
      "tool_input": {
        "command": "*** Begin Patch\n*** Update File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _request = rx.recv().expect("captured request should arrive");
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("deny outcome should serialize");
    assert_eq!(json["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        json["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .expect("denial reason should be text")
            .contains("Declare matching reservation.")
    );
    assert!(
        !paths
            .runtime_dir
            .join("write-intents/7331/64656e792d617574686f72697a6174696f6e.json")
            .exists(),
        "a definitive denial must not remain replayable"
    );
}

#[test]
fn pre_tool_use_denies_new_dependency_shadowing_python_root_before_authorize() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    fs::write(
        repo_root.join("pyproject.toml"),
        r#"
[project]
name = "example-project"
dependencies = ["langchain-core>=0.3"]
"#,
    )
    .expect("pyproject should write");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"status":"ok","prompt_text":"Nearby Activity\n- unexpected context"}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = serde_json::json!({
        "agent_id": "s1",
        "cwd": repo_root,
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "tool_input": {
            "command": "*** Begin Patch\n*** Add File: langchain_core/__init__.py\n+\"\"\"local shim\"\"\"\n*** End Patch\n"
        }
    })
    .to_string();

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        &input,
    );

    assert!(
        output.status.success(),
        "stateful hook should return a structured denial, not crash: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"permissionDecision\":\"deny\""));
    assert!(stdout.contains("dependency shadowing guard"));
    assert!(stdout.contains("langchain_core"));
    assert!(stdout.contains("langchain-core"));
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "shadowing guard should not post /v1/context/render or /v1/authorize"
    );
}

#[test]
fn pre_tool_use_apply_patch_delete_posts_delete_file_action() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(
        r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
    );
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "agent_id": "s1",
      "cwd": "/repo",
      "hook_event_name": "PreToolUse",
      "tool_name": "apply_patch",
      "tool_input": {
        "command": "*** Begin Patch\n*** Delete File: src/auth.ts\n*** End Patch\n"
      }
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "pre-tool-use"],
        input,
    );

    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("\"action\":\"delete_file\""));
    assert!(request.contains("\"path\":\"src/auth.ts\""));
}

#[test]
fn session_start_posts_session_register() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) = spawn_fake_stateful_server(r#"{"status":"ok"}"#);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "agent_id": "s1",
      "hook_event_name": "SessionStart"
    }"#;

    let output = run_hook_subprocess(
        &repo_root,
        &paths,
        &["hook", "codex", "session-start"],
        input,
    );
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v2/session/register HTTP/1.1"));
    assert!(request.contains("\"agent_id\":\"s1\""));
    assert_eq!(
        request_json_body(&request)["workspace"]["workspace_id"],
        workspace_id_for_enabled_repo(&paths, &repo_root).expect("repo workspace id should load")
    );
}

#[test]
fn post_tool_use_in_disabled_repo_noops_without_outbox() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    fs::create_dir_all(temp_root.join(".git")).expect("git marker should write");

    let input = r#"{
      "agent_id": "s1",
      "hook_event_name": "PostToolUse",
      "tool_name": "Bash",
      "tool_input": {"command": "rg auth src"}
    }"#;

    handle_post_tool_use_in_repo(input, temp_root).expect("disabled repo should no-op");

    assert!(!temp_root.join(".stateful_core/outbox/s1.jsonl").exists());
}

#[test]
fn stop_posts_activity_finalize() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_root = temp.path();
    let paths = GlobalPaths::new(temp_root.join("home"));
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    enable_test_repo(&paths, &repo_root);
    let (runtime, rx) =
        spawn_fake_stateful_server_sequence(vec![r#"{"status":"ok"}"#, r#"{"status":"ok"}"#]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let input = r#"{
      "agent_id": "s1",
      "hook_event_name": "Stop"
    }"#;

    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "codex", "stop"], input);
    assert!(
        output.status.success(),
        "stateful hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v2/activity/finalize HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["agent"]["agent_id"], "s1");
    assert_eq!(
        body["workspace"]["workspace_id"],
        workspace_id_for_enabled_repo(&paths, &repo_root).expect("repo workspace id should load")
    );
    assert!(
        body["payload"].is_null(),
        "unspecified Stop must use automatic fallback"
    );

    let explicit = r#"{
      "agent_id": "s1",
      "hook_event_name": "Stop",
      "handoff": {"status":"done","summary":"complete"}
    }"#;
    let output = run_hook_subprocess(&repo_root, &paths, &["hook", "codex", "stop"], explicit);
    assert!(
        output.status.success(),
        "explicit Stop hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let explicit = request_json_body(&rx.recv().expect("explicit finalize should arrive"));
    assert_eq!(
        explicit["payload"],
        serde_json::json!({"status":"done","summary":"complete"})
    );
}

fn read_http_request_maybe_body(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut byte = [0_u8; 1];
    while !buffer.ends_with(b"\r\n\r\n") {
        stream
            .read_exact(&mut byte)
            .expect("request header byte should read");
        buffer.push(byte[0]);
    }

    let headers = String::from_utf8(buffer.clone()).expect("headers should be utf8");
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .map(|value| value.parse::<usize>().expect("content length should parse"))
        .unwrap_or(0);

    let mut body = vec![0_u8; content_length];
    if content_length > 0 {
        stream
            .read_exact(&mut body)
            .expect("request body should read");
        buffer.extend_from_slice(&body);
    }

    String::from_utf8(buffer).expect("request should be utf8")
}

fn request_json_body(request: &str) -> serde_json::Value {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .expect("request should contain a body separator");
    serde_json::from_str(body).expect("request body should be json")
}

fn is_v2_runtime_identity(response: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(response)
        .ok()
        .is_some_and(|body| {
            body["protocol_version"] == "stateful.v2"
                && body.get("journal_schema_version").is_some()
                && body.get("pid").is_some()
        })
}

fn fake_v2_runtime_identity() -> &'static str {
    r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":42,"workspace_id":"w1","workspace_version":1,"capabilities":["presence"]}"#
}

fn migrate_fake_authorize_response(request: &str, response: &str) -> String {
    if !request.contains("POST /v2/authorize HTTP/1.1") {
        return response.to_string();
    }
    let Ok(body) = serde_json::from_str::<serde_json::Value>(response) else {
        return response.to_string();
    };
    let Some(decision) = body["decision"].as_str() else {
        return response.to_string();
    };
    if decision == "deny" {
        return body.to_string();
    }
    let request_body = request_json_body(request);
    serde_json::json!({
        "intent_id": request_body["payload"]["operation_id"],
        "fence_ids": [],
        "decision": {
            "decision": decision,
            "reason_code": body["reason_code"].as_str().unwrap_or("authorized"),
            "message": body["message"],
            "required_next_action": body["required_next_action"],
        }
    })
    .to_string()
}

fn enable_test_repo(paths: &GlobalPaths, repo_root: &std::path::Path) {
    fs::create_dir_all(repo_root.join(".git")).expect("git marker should write");
    enable_repo(paths, repo_root).expect("repo should enable");
}

fn fake_current_response(request: &str) -> String {
    let resource = request
        .strip_prefix("GET /v1/current?resource=")
        .and_then(|rest| rest.split_once(" HTTP/1.1").map(|(resource, _)| resource))
        .unwrap_or("src/auth.ts");
    serde_json::json!({
        "status": "ok",
        "current": {},
        "items": [{
            "kind": "reservation",
            "freshness": "live",
            "resource": resource,
            "purpose": "Fix auth validation behavior.",
            "agent_id": "s1",
            "workspace_id": "w1"
        }]
    })
    .to_string()
}

fn spawn_fake_stateful_server(
    actual_response: &'static str,
) -> (ServerRuntime, mpsc::Receiver<String>) {
    spawn_fake_stateful_server_sequence(vec![actual_response])
}

fn spawn_fake_stateful_server_sequence(
    actual_responses: Vec<&'static str>,
) -> (ServerRuntime, mpsc::Receiver<String>) {
    spawn_fake_stateful_server_sequence_with_current(actual_responses, None)
}

fn spawn_fake_stateful_server_sequence_with_current(
    actual_responses: Vec<&'static str>,
    current_response: Option<&'static str>,
) -> (ServerRuntime, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut actual_responses = VecDeque::from(actual_responses);
        while !actual_responses.is_empty() {
            let (mut stream, _) = listener.accept().expect("connection should arrive");
            let request = read_http_request_maybe_body(&mut stream);
            if request.contains("GET /health HTTP/1.1") {
                write_json_response(&mut stream, r#"{"status":"ok"}"#);
            } else if request.starts_with("GET /v2/runtime/identity?") {
                if actual_responses
                    .front()
                    .is_some_and(|response| is_v2_runtime_identity(response))
                {
                    let response = actual_responses
                        .pop_front()
                        .expect("identity response should exist");
                    tx.send(request).expect("request should send to test");
                    write_json_response(&mut stream, response);
                } else {
                    write_json_response(&mut stream, fake_v2_runtime_identity());
                }
            } else if request.starts_with("GET /v1/current") {
                let response = current_response
                    .map(str::to_string)
                    .unwrap_or_else(|| fake_current_response(&request));
                write_json_response(&mut stream, &response);
            } else if request.contains("GET /v1/runtime/identity HTTP/1.1") {
                write_json_response(
                    &mut stream,
                    r#"{"status":"ok","pid":42,"protocol_version":"stateful.v1","capabilities":["authorize.write_directory"]}"#,
                );
            } else {
                let response = actual_responses
                    .pop_front()
                    .expect("response should exist while loop is active");
                let response = migrate_fake_authorize_response(&request, response);
                tx.send(request).expect("request should send to test");
                write_json_response(&mut stream, &response);
            }
        }
    });

    (
        ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42),
        rx,
    )
}

fn spawn_v2_started_deny_server() -> (ServerRuntime, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let request = read_http_request_maybe_body(&mut stream);
            if request.starts_with("GET /v2/runtime/identity?") {
                write_json_response(&mut stream, fake_v2_runtime_identity());
                continue;
            }
            tx.send(request.clone())
                .expect("request should send to test");
            let body = if request.starts_with("POST /v2/authorize ") {
                r#"{"intent_id":"intent-denied-1","decision":{"decision":"deny","reason_code":"denied","message":"blocked"}}"#
            } else {
                r#"{"status":"completed"}"#
            };
            write_ok_json_response(&mut stream, body);
        }
    });
    (
        ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42),
        rx,
    )
}

fn spawn_fake_stateful_server_with_runtime_pid(
    actual_response: &'static str,
    runtime_pid: Arc<AtomicU32>,
) -> (ServerRuntime, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let request = read_http_request_maybe_body(&mut stream);
            if request.starts_with("GET /v2/runtime/identity?") {
                let response = serde_json::json!({
                    "protocol_version": "stateful.v2",
                    "journal_schema_version": 2,
                    "coordination_mode": "awareness",
                    "pid": runtime_pid.load(Ordering::Acquire),
                    "workspace_id": "w1",
                    "workspace_version": 1,
                    "capabilities": ["presence"],
                })
                .to_string();
                write_json_response(&mut stream, &response);
                continue;
            }
            if request.contains("GET /health HTTP/1.1") {
                write_json_response(&mut stream, r#"{"status":"ok"}"#);
                continue;
            }
            tx.send(request.clone())
                .expect("request should send to test");
            let response = migrate_fake_authorize_response(&request, actual_response);
            write_json_response(&mut stream, &response);
            break;
        }
    });

    (
        ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 0),
        rx,
    )
}

fn spawn_server_dropping_presence_update() -> (ServerRuntime, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let request = read_http_request_maybe_body(&mut stream);
            if request.starts_with("GET /v2/runtime/identity?") {
                write_json_response(&mut stream, fake_v2_runtime_identity());
                continue;
            }
            tx.send(request.clone())
                .expect("request should send to test");
            if request.starts_with("POST /v2/presence/update ") {
                break;
            }
            write_json_response(&mut stream, r#"{"status":"recorded"}"#);
        }
    });
    (
        ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42),
        rx,
    )
}

fn spawn_fake_stateful_server_dropping_authorize() -> (ServerRuntime, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let request = read_http_request_maybe_body(&mut stream);
            if request.contains("GET /health HTTP/1.1") {
                write_json_response(&mut stream, r#"{"status":"ok"}"#);
            } else if request.starts_with("GET /v2/runtime/identity?") {
                write_json_response(&mut stream, fake_v2_runtime_identity());
            } else if request.starts_with("GET /v1/current") {
                let response = fake_current_response(&request);
                write_json_response(&mut stream, &response);
            } else if request.contains("GET /v1/runtime/identity HTTP/1.1") {
                write_json_response(
                    &mut stream,
                    r#"{"status":"ok","pid":42,"protocol_version":"stateful.v1","capabilities":["authorize.write_directory"]}"#,
                );
            } else {
                tx.send(request).expect("request should send to test");
                drop(stream);
                break;
            }
        }
    });

    (
        ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42),
        rx,
    )
}

fn spawn_server_dropping_route(route: &'static str) -> (ServerRuntime, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let request = read_http_request_maybe_body(&mut stream);
            if request.starts_with("GET /v2/runtime/identity?") {
                write_json_response(&mut stream, fake_v2_runtime_identity());
                continue;
            }
            tx.send(request.clone())
                .expect("request should send to test");
            if request.starts_with(route) {
                drop(stream);
                break;
            }
            write_json_response(&mut stream, r#"{"status":"ok"}"#);
        }
    });
    (
        ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42),
        rx,
    )
}

fn run_hook_subprocess(
    repo_root: &std::path::Path,
    paths: &GlobalPaths,
    args: &[&str],
    input: &str,
) -> std::process::Output {
    run_hook_subprocess_from(repo_root, paths, args, input)
}

fn run_hook_subprocess_with_extra_env(
    repo_root: &std::path::Path,
    paths: &GlobalPaths,
    args: &[&str],
    input: &str,
    extra_env: &[(&str, &str)],
) -> std::process::Output {
    run_hook_subprocess_from_with_extra_env(repo_root, paths, args, input, extra_env)
}

fn run_hook_subprocess_from(
    cwd: &Path,
    paths: &GlobalPaths,
    args: &[&str],
    input: &str,
) -> std::process::Output {
    run_hook_subprocess_from_with_extra_env(cwd, paths, args, input, &[])
}

fn hook_input_with_test_operation_id(args: &[&str], input: &str) -> String {
    if args.last() != Some(&"pre-tool-use") {
        return input.to_string();
    }
    let Ok(mut input) = serde_json::from_str::<serde_json::Value>(input) else {
        return input.to_string();
    };
    let is_write = input["tool_name"].as_str().is_some_and(|tool_name| {
        ["apply_patch", "edit", "write"]
            .iter()
            .any(|candidate| tool_name.eq_ignore_ascii_case(candidate))
    });
    if is_write && input.get("tool_use_id").is_none() && input.get("call_id").is_none() {
        input["tool_use_id"] = serde_json::json!("test-write-operation");
    }
    input.to_string()
}

fn run_hook_subprocess_from_with_extra_env(
    cwd: &Path,
    paths: &GlobalPaths,
    args: &[&str],
    input: &str,
    extra_env: &[(&str, &str)],
) -> std::process::Output {
    let input = hook_input_with_test_operation_id(args, input);
    let fixture_runtime = fs::read_to_string(&paths.server_json)
        .ok()
        .and_then(|contents| serde_json::from_str::<ServerRuntime>(&contents).ok());
    let has_runtime_override = extra_env
        .iter()
        .any(|(key, _)| *key == "STATEFUL_SERVER_URL" || *key == "STATEFUL_SERVER_TOKEN");
    let mut command = Command::new(env!("CARGO_BIN_EXE_stateful"));
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("STATEFUL_HOME", &paths.home);
    if !has_runtime_override {
        if let Some(runtime) = fixture_runtime {
            command
                .env("STATEFUL_SERVER_URL", runtime.base_url)
                .env("STATEFUL_SERVER_TOKEN", runtime.token);
        }
    }
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("stateful hook should spawn");
    let mut stdin = child.stdin.take().expect("stdin should be piped");
    stdin
        .write_all(input.as_bytes())
        .expect("hook input should write");
    drop(stdin);
    child
        .wait_with_output()
        .expect("stateful hook should complete")
}

fn write_json_response(stream: &mut std::net::TcpStream, body: &str) {
    let status = if body.contains(r#""decision":"deny""#) {
        "403 Forbidden"
    } else {
        "200 OK"
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .expect("response should write");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("response should close");
}

fn write_ok_json_response(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .expect("response should write");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("response should close");
}
