use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use stateful_cli::{
    GlobalPaths, HookOutcome, ServerRuntime, enable_repo, handle_omp_post_tool_use_with_runtime,
    handle_omp_pre_tool_use_with_runtime, handle_omp_session_start_with_runtime,
    handle_pre_tool_use_in_repo, write_global_runtime_file,
};

fn runtime_and_requests(responses: Vec<Value>) -> (ServerRuntime, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let address = listener.local_addr().expect("listener should have address");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for mut payload in responses {
            let (mut stream, _) = listener.accept().expect("request should connect");
            let request = read_request(&mut stream);
            let body = request_body(&request);
            let request_id = body
                .as_ref()
                .and_then(|body| body.get("request_id"))
                .cloned()
                .unwrap_or(Value::Null);
            if payload.get("task_id").and_then(Value::as_str) == Some("$request_task_id") {
                payload["task_id"] = body
                    .as_ref()
                    .and_then(|body| body.get("task_id"))
                    .cloned()
                    .expect("task command should carry a task_id");
            }
            sender.send(request).expect("request should send");
            write_response(
                &mut stream,
                &json!({
                    "protocol_version": "stateful.v2",
                    "contract_revision": "lease-1",
                    "request_id": request_id,
                    "payload": payload,
                }),
            );
        }
    });
    (
        ServerRuntime::new(
            format!("http://{address}"),
            "test-token",
            "workspace",
            std::process::id(),
            stateful_cli::process_start_identity_for_pid(std::process::id())
                .expect("test process identity should resolve"),
        ),
        receiver,
    )
}

fn runtime_and_live_task_requests() -> (ServerRuntime, mpsc::Receiver<(Instant, String)>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let address = listener.local_addr().expect("listener should have address");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(mut stream) = incoming else {
                break;
            };
            let request = read_request(&mut stream);
            let body = request_body(&request);
            let request_id = body
                .as_ref()
                .and_then(|body| body.get("request_id"))
                .cloned()
                .unwrap_or(Value::Null);
            let task_id = body
                .as_ref()
                .and_then(|body| body.get("task_id"))
                .cloned()
                .unwrap_or(Value::Null);
            let payload = if request.starts_with("POST /v2/tasks/finalize") {
                json!({ "task_id": task_id, "status": "completed", "draining": false })
            } else {
                json!({ "task_id": task_id, "status": "active", "draining": false })
            };
            if sender.send((Instant::now(), request)).is_err() {
                break;
            }
            write_response(
                &mut stream,
                &json!({
                    "protocol_version": "stateful.v2",
                    "contract_revision": "lease-1",
                    "request_id": request_id,
                    "payload": payload,
                }),
            );
        }
    });
    (
        ServerRuntime::new(
            format!("http://{address}"),
            "test-token",
            "workspace",
            std::process::id(),
            stateful_cli::process_start_identity_for_pid(std::process::id())
                .expect("test process identity should resolve"),
        ),
        receiver,
    )
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let count = stream.read(&mut buffer).expect("request should read");
        assert!(count > 0, "request closed before headers");
        bytes.extend_from_slice(&buffer[..count]);
        let Some(headers_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&bytes[..headers_end]);
        let length = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        if bytes.len() >= headers_end + 4 + length {
            return String::from_utf8(bytes).expect("request should be UTF-8");
        }
    }
}

fn request_body(request: &str) -> Option<Value> {
    request
        .split_once("\r\n\r\n")
        .and_then(|(_, body)| serde_json::from_str(body).ok())
}

fn write_response(stream: &mut TcpStream, body: &Value) {
    let body = body.to_string();
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .as_bytes(),
        )
        .expect("response should write");
}

fn run_codex_hook(
    repo: &Path,
    paths: &GlobalPaths,
    command: &str,
    input: &str,
) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["hook", "codex", command])
        .current_dir(repo)
        .env("STATEFUL_HOME", &paths.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Codex hook should start");
    child
        .stdin
        .take()
        .expect("Codex hook stdin should open")
        .write_all(input.as_bytes())
        .expect("Codex hook input should write");
    child.wait_with_output().expect("Codex hook should finish")
}

fn lifecycle(cwd: &Path) -> Value {
    json!({
        "type": "agent_start",
        "runtime": "omp",
        "version": "17.2.3",
        "cwd": cwd,
        "sessionId": "session-1",
        "leafAgentId": "root",
    })
}

fn task_payload(
    cwd: &Path,
    task_id: &str,
    event_type: &str,
    tool_name: &str,
    call_id: &str,
    input: Value,
) -> Value {
    json!({
        "type": event_type,
        "runtime": "omp",
        "version": "17.2.3",
        "cwd": cwd,
        "sessionId": "session-1",
        "leafAgentId": "root",
        "task_id": task_id,
        "toolName": tool_name,
        "toolCallId": call_id,
        "input": input,
    })
}

#[test]
fn omp_queued_writer_rereads_before_retrying_activation_and_before_permit() {
    let workspace = tempfile::tempdir().expect("workspace should create");
    let file = workspace.path().join("note.txt");
    std::fs::write(&file, "before\n").expect("fixture should write");
    let (runtime, requests) = runtime_and_requests(vec![
        json!({ "task_id": "$request_task_id", "status": "active", "draining": false }),
        json!({ "read_id": "ignored", "status": "started" }),
        json!({ "read_id": "ignored", "status": "completed", "evidence_id": "evidence" }),
        json!({ "status": "queued", "batch_id": "batch-1" }),
        json!({ "batch_id": "batch-1", "state": "offered", "version": 1, "offer_id": "offer-1" }),
        json!({ "batch_id": "batch-1", "active": false }),
        json!({ "read_id": "ignored", "status": "started" }),
        json!({ "read_id": "ignored", "status": "completed", "evidence_id": "evidence" }),
        json!({ "status": "queued", "batch_id": "batch-1" }),
        json!({ "batch_id": "batch-1", "state": "offered", "version": 1, "offer_id": "offer-1" }),
        json!({ "batch_id": "batch-1", "active": true }),
        json!({ "read_id": "ignored", "status": "started" }),
        json!({ "read_id": "ignored", "status": "completed", "evidence_id": "evidence" }),
        json!({ "status": "ready", "attempt_id": "attempt-1", "permit_id": "permit-1", "lease_batch_ids": ["batch-1"] }),
    ]);
    let task =
        handle_omp_session_start_with_runtime(&lifecycle(workspace.path()).to_string(), &runtime)
            .expect("task start should succeed");
    let read_exact = |call_id: &str| {
        let read = task_payload(
            workspace.path(),
            &task.task_id,
            "tool_call",
            "read",
            call_id,
            json!({ "path": "note.txt:raw" }),
        );
        assert!(matches!(
            handle_omp_pre_tool_use_with_runtime(
                &read.to_string(),
                Some(&runtime),
                Some(workspace.path()),
                Some(workspace.path())
            )
            .expect("read start should succeed"),
            stateful_cli::OmpHookOutcome::Allow
        ));
        let mut read_result = task_payload(
            workspace.path(),
            &task.task_id,
            "tool_result",
            "read",
            call_id,
            json!({ "path": "note.txt:raw" }),
        );
        read_result["content"] = json!([{ "type": "text", "text": "before\n" }]);
        read_result["details"] = json!({
            "displayContent": { "text": "before\n" },
            "fileSize": 7,
            "meta": { "source": { "type": "path", "value": file } }
        });
        read_result["isError"] = json!(false);
        handle_omp_post_tool_use_with_runtime(&read_result.to_string(), &runtime)
            .expect("read complete should succeed");
    };
    let write = |call_id: &str| {
        handle_omp_pre_tool_use_with_runtime(
            &task_payload(
                workspace.path(),
                &task.task_id,
                "tool_call",
                "write",
                call_id,
                json!({ "path": "note.txt", "content": "after\n" }),
            )
            .to_string(),
            Some(&runtime),
            Some(workspace.path()),
            Some(workspace.path()),
        )
        .expect("queued write should be handled")
    };

    read_exact("read-1");
    let inactive = write("write-1");
    let stateful_cli::OmpHookOutcome::Block { reason } = inactive else {
        panic!("inactive activation must block");
    };
    assert!(reason.starts_with("reread_required:"));
    assert!(!reason.contains("activated"));

    read_exact("read-2");
    let activated = write("write-2");
    assert!(matches!(
        activated,
        stateful_cli::OmpHookOutcome::Block { ref reason }
            if reason.starts_with("reread_required: lease activated")
    ));

    read_exact("read-3");
    assert!(matches!(
        write("write-3"),
        stateful_cli::OmpHookOutcome::AllowWithWriteAttempt { ref attempt_id, ref permit_id }
            if attempt_id == "attempt-1" && permit_id == "permit-1"
    ));
    assert_eq!(
        std::fs::read_to_string(&file).expect("fixture should read"),
        "before\n"
    );

    let observed = (0..14)
        .map(|_| requests.recv().expect("route request should arrive"))
        .collect::<Vec<_>>();
    for (index, route) in [
        "POST /v2/tasks/start HTTP/1.1",
        "POST /v2/reads/start HTTP/1.1",
        "POST /v2/reads/complete HTTP/1.1",
        "POST /v2/writes/prepare HTTP/1.1",
        "GET /v2/lease-requests/batch-1?",
        "POST /v2/leases/activate HTTP/1.1",
        "POST /v2/reads/start HTTP/1.1",
        "POST /v2/reads/complete HTTP/1.1",
        "POST /v2/writes/prepare HTTP/1.1",
        "GET /v2/lease-requests/batch-1?",
        "POST /v2/leases/activate HTTP/1.1",
        "POST /v2/reads/start HTTP/1.1",
        "POST /v2/reads/complete HTTP/1.1",
        "POST /v2/writes/prepare HTTP/1.1",
    ]
    .iter()
    .enumerate()
    {
        assert!(
            observed[index].starts_with(route),
            "unexpected route at {index}"
        );
    }
    assert_ne!(
        request_body(&observed[5]).expect("first activation body")["request_id"],
        request_body(&observed[10]).expect("second activation body")["request_id"]
    );
}

#[test]
fn denied_prepare_leaves_target_unmutated() {
    let workspace = tempfile::tempdir().expect("workspace should create");
    let file = workspace.path().join("note.txt");
    std::fs::write(&file, "before\n").expect("fixture should write");
    let (runtime, requests) = runtime_and_requests(vec![
        json!({ "task_id": "$request_task_id", "status": "active", "draining": false }),
        json!({ "status": "denied", "reason_code": "evidence_missing" }),
    ]);
    let task =
        handle_omp_session_start_with_runtime(&lifecycle(workspace.path()).to_string(), &runtime)
            .expect("task start should succeed");
    let write = task_payload(
        workspace.path(),
        &task.task_id,
        "tool_call",
        "write",
        "write-denied",
        json!({ "path": "note.txt", "content": "after\n" }),
    );
    let outcome = handle_omp_pre_tool_use_with_runtime(
        &write.to_string(),
        Some(&runtime),
        Some(workspace.path()),
        Some(workspace.path()),
    )
    .expect("denied write should be handled");
    assert!(matches!(
        outcome,
        stateful_cli::OmpHookOutcome::Block { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(&file).expect("fixture should read"),
        "before\n"
    );
    assert!(
        requests
            .recv()
            .expect("start route should arrive")
            .starts_with("POST /v2/tasks/start HTTP/1.1")
    );
    assert!(
        requests
            .recv()
            .expect("prepare route should arrive")
            .starts_with("POST /v2/writes/prepare HTTP/1.1")
    );
}

#[test]
fn prepared_write_completes_with_correlated_terminal_payload() {
    let workspace = tempfile::tempdir().expect("workspace should create");
    let file = workspace.path().join("note.txt");
    std::fs::write(&file, "before\n").expect("fixture should write");
    let (runtime, requests) = runtime_and_requests(vec![
        json!({ "task_id": "$request_task_id", "status": "active", "draining": false }),
        json!({ "status": "ready", "attempt_id": "attempt-1", "permit_id": "permit-1", "lease_batch_ids": ["lease-1"] }),
        json!({ "attempt_id": "attempt-1", "status": "completed" }),
    ]);
    let task =
        handle_omp_session_start_with_runtime(&lifecycle(workspace.path()).to_string(), &runtime)
            .expect("task start should succeed");
    let write = task_payload(
        workspace.path(),
        &task.task_id,
        "tool_call",
        "write",
        "write-ready",
        json!({ "path": "note.txt", "content": "after\n" }),
    );
    let outcome = handle_omp_pre_tool_use_with_runtime(
        &write.to_string(),
        Some(&runtime),
        Some(workspace.path()),
        Some(workspace.path()),
    )
    .expect("prepared write should be allowed");
    assert!(matches!(
        outcome,
        stateful_cli::OmpHookOutcome::AllowWithWriteAttempt { ref attempt_id, ref permit_id }
            if attempt_id == "attempt-1" && permit_id == "permit-1"
    ));

    std::fs::write(&file, "after\n").expect("native write fixture should mutate");
    let mut result = task_payload(
        workspace.path(),
        &task.task_id,
        "tool_result",
        "write",
        "write-ready",
        json!({ "path": "note.txt", "content": "after\n" }),
    );
    result["isError"] = json!(false);
    result["details"] = json!({ "resolvedPath": file });
    result["content"] =
        json!([{ "type": "text", "text": "Successfully wrote 6 bytes to note.txt" }]);
    result["attempt_id"] = json!("attempt-1");
    result["permit_id"] = json!("permit-1");
    handle_omp_post_tool_use_with_runtime(&result.to_string(), &runtime)
        .expect("write completion should succeed");
    assert_eq!(
        std::fs::read_to_string(&file).expect("fixture should read"),
        "after\n"
    );
    assert!(
        requests
            .recv()
            .expect("start route should arrive")
            .starts_with("POST /v2/tasks/start HTTP/1.1")
    );
    assert!(
        requests
            .recv()
            .expect("prepare route should arrive")
            .starts_with("POST /v2/writes/prepare HTTP/1.1")
    );
    assert!(
        requests
            .recv()
            .expect("complete route should arrive")
            .starts_with("POST /v2/writes/complete HTTP/1.1")
    );
}

fn codex_task_id(session_id: &str, turn_id: &str) -> String {
    let mut bytes = b"codex-task".to_vec();
    for field in [session_id, turn_id] {
        bytes.push(0);
        bytes.extend_from_slice(field.as_bytes());
    }
    format!("codex-task-{}", stateful_core::digest_bytes(&bytes).value)
}

fn codex_bash(command: &str) -> String {
    json!({
        "session_id": "session-1",
        "turn_id": "turn-1",
        "tool_name": "Bash",
        "tool_input": { "command": command },
    })
    .to_string()
}

#[test]
fn codex_allows_only_a_matching_owned_mutation_wrapper() {
    let executable = std::env::current_exe().expect("test executable should resolve");
    let task_id = codex_task_id("session-1", "turn-1");
    let command = format!(
        "'{}' sandbox run --fs mutation --task-id '{}' --agent-id 'codex:session-1' --operation '{{\"kind\":\"update\",\"path\":\"README.md\"}}' --command 'printf ok'",
        executable.display(),
        task_id,
    );
    assert_eq!(
        handle_pre_tool_use_in_repo(&codex_bash(&command), ".")
            .expect("matching wrapper should validate"),
        HookOutcome::Allow
    );

    let mismatched = command.replace(&task_id, "another-task");
    assert!(matches!(
        handle_pre_tool_use_in_repo(&codex_bash(&mismatched), ".")
            .expect("mismatched wrapper should produce a denial"),
        HookOutcome::Deny { .. }
    ));
}

#[test]
fn codex_rejects_outer_shell_chaining_around_wrappers() {
    let executable = std::env::current_exe().expect("test executable should resolve");
    let command = format!("'{}' status; printf bypass", executable.display());
    assert!(matches!(
        handle_pre_tool_use_in_repo(&codex_bash(&command), ".")
            .expect("chained wrapper should produce a denial"),
        HookOutcome::Deny { .. }
    ));
}

#[test]
fn omp_cli_hook_discovers_the_global_runtime() {
    let temp = tempfile::tempdir().expect("temp directory should create");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).expect("git marker should create");
    let paths = GlobalPaths::new(temp.path().join("stateful"));
    enable_repo(&paths, &repo).expect("repo should enable");
    let (runtime, requests) = runtime_and_requests(vec![json!({
        "task_id": "$request_task_id",
        "status": "active",
        "draining": false,
    })]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime should write");

    let mut child = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["hook", "omp", "session-start"])
        .current_dir(&repo)
        .env("STATEFUL_HOME", &paths.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("OMP hook should start");
    child
        .stdin
        .take()
        .expect("OMP hook stdin should open")
        .write_all(lifecycle(&repo).to_string().as_bytes())
        .expect("OMP lifecycle payload should write");
    let output = child.wait_with_output().expect("OMP hook should finish");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).expect("OMP output should decode")["decision"],
        "allow"
    );
    assert!(
        requests
            .recv()
            .expect("task start route should arrive")
            .starts_with("POST /v2/tasks/start HTTP/1.1")
    );
}

#[test]
fn codex_heartbeat_helper_refreshes_while_owner_process_is_live() {
    let temp = tempfile::tempdir().expect("temp directory should create");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).expect("git marker should create");
    let paths = GlobalPaths::new(temp.path().join("stateful"));
    enable_repo(&paths, &repo).expect("repo should enable");
    let (runtime, requests) = runtime_and_requests(vec![json!({
        "task_id": "$request_task_id",
        "status": "active",
        "draining": false,
    })]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime should write");

    let session_id = "heartbeat-session";
    let turn_id = "heartbeat-turn";
    let task_id = codex_task_id(session_id, turn_id);
    let owner = json!({
        "task_id": task_id,
        "agent_id": format!("codex:{session_id}"),
        "workspace_id": runtime.workspace_id,
        "session_id": session_id,
        "turn_id": turn_id,
        "pid": std::process::id(),
        "process_start_identity": stateful_cli::process_start_identity_for_pid(std::process::id())
            .expect("test process identity should resolve"),
        "heartbeat_enabled": true,
        "task_start_observed_at": "2026-08-02T00:00:00Z",
        "task_start_input": {
            "next_action": "Codex root turn active",
            "settings": {
                "heartbeat_interval_seconds": 1,
                "inactivity_timeout_seconds": 5,
                "lease_expiry_seconds": 60,
                "offer_ttl_seconds": 120
            },
            "expires_at": "2026-08-02T00:00:05Z"
        }
    });
    let owner_directory = repo.join(".stateful_core/runtime/tasks");
    std::fs::create_dir_all(&owner_directory).expect("owner directory should create");
    let owner_path = owner_directory.join(format!("{task_id}.json"));
    std::fs::write(&owner_path, owner.to_string()).expect("owner record should write");

    let mut child = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["hook", "codex", "heartbeat"])
        .current_dir(&repo)
        .env("STATEFUL_HOME", &paths.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Codex heartbeat helper should start");
    child
        .stdin
        .take()
        .expect("heartbeat helper stdin should open")
        .write_all(
            json!({
                "repo_root": repo.canonicalize().expect("repo should canonicalize"),
                "owner": owner,
                "settings": {
                    "heartbeat_interval_seconds": 1,
                    "inactivity_timeout_seconds": 5,
                    "lease_expiry_seconds": 60,
                    "offer_ttl_seconds": 120,
                },
            })
            .to_string()
            .as_bytes(),
        )
        .expect("heartbeat helper payload should write");

    let request = requests
        .recv_timeout(Duration::from_secs(2))
        .expect("heartbeat route should arrive");
    assert!(request.starts_with("POST /v2/tasks/heartbeat HTTP/1.1"));
    assert_eq!(
        request_body(&request).expect("heartbeat body should decode")["task_id"],
        task_id
    );
    std::fs::remove_file(owner_path).expect("owner record should remove");
    let output = child
        .wait_with_output()
        .expect("heartbeat helper should stop");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn codex_duplicate_submit_reuses_the_exact_task_start_request() {
    let temp = tempfile::tempdir().expect("temp directory should create");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).expect("git marker should create");
    std::fs::create_dir_all(repo.join(".stateful")).expect("settings directory should create");
    std::fs::write(
        repo.join(".stateful/config.yml"),
        "heartbeat_interval_seconds: 20\ninactivity_timeout_seconds: 60\n",
    )
    .expect("settings should write");
    let paths = GlobalPaths::new(temp.path().join("stateful"));
    enable_repo(&paths, &repo).expect("repo should enable");
    let (runtime, requests) = runtime_and_live_task_requests();
    write_global_runtime_file(&paths, &runtime).expect("global runtime should write");
    let input = json!({ "session_id": "replay-session", "turn_id": "replay-turn" }).to_string();

    assert!(
        run_codex_hook(&repo, &paths, "user-prompt-submit", &input)
            .status
            .success(),
        "first task start should succeed"
    );
    assert!(
        run_codex_hook(&repo, &paths, "user-prompt-submit", &input)
            .status
            .success(),
        "replayed task start should succeed"
    );

    let mut starts = Vec::new();
    while starts.len() < 2 {
        let (_, request) = requests
            .recv_timeout(Duration::from_secs(2))
            .expect("task start request should arrive");
        if request.starts_with("POST /v2/tasks/start HTTP/1.1") {
            starts.push(request_body(&request).expect("task start body should decode"));
        }
    }
    assert_eq!(starts[0], starts[1]);
    let owner_path = repo.join(".stateful_core/runtime/tasks").join(format!(
        "{}.json",
        codex_task_id("replay-session", "replay-turn")
    ));
    let mut owner: Value = serde_json::from_str(
        &std::fs::read_to_string(&owner_path).expect("owner record should read"),
    )
    .expect("owner record should decode");
    assert_eq!(owner["task_start_observed_at"], starts[0]["observed_at"]);
    assert_eq!(owner["task_start_input"], starts[0]["payload"]);

    owner["pid"] = json!(u32::MAX);
    std::fs::write(
        &owner_path,
        serde_json::to_vec(&owner).expect("owner should encode"),
    )
    .expect("owner mismatch should write");
    let persisted: Value =
        serde_json::from_slice(&std::fs::read(&owner_path).expect("mismatched owner should read"))
            .expect("mismatched owner should decode");
    assert_eq!(persisted["pid"], json!(u32::MAX));
    let mismatched = run_codex_hook(&repo, &paths, "user-prompt-submit", &input);
    assert!(mismatched.status.success());
    assert!(mismatched.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&mismatched.stderr)
            .contains("existing Codex owner does not match the replayed task")
    );
    assert!(
        requests
            .try_iter()
            .all(|(_, request)| !request.starts_with("POST /v2/tasks/start HTTP/1.1"))
    );
    std::fs::remove_file(owner_path).expect("owner record should remove");
}

#[test]
fn codex_exec_hook_keeps_heartbeating_past_the_inactivity_timeout() {
    let temp = tempfile::tempdir().expect("temp directory should create");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).expect("git marker should create");
    let paths = GlobalPaths::new(temp.path().join("stateful"));
    enable_repo(&paths, &repo).expect("repo should enable");
    let (runtime, requests) = runtime_and_live_task_requests();
    write_global_runtime_file(&paths, &runtime).expect("global runtime should write");
    let executable = std::path::PathBuf::from(env!("CARGO_BIN_EXE_stateful"));
    let started_at = Instant::now();
    let shell_command = format!(
        "exec '{}' hook codex user-prompt-submit",
        executable.display()
    );
    let mut child = Command::new("sh")
        .args(["-c", &shell_command])
        .current_dir(&repo)
        .env("STATEFUL_HOME", &paths.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("exec hook should start");
    child
        .stdin
        .take()
        .expect("exec hook stdin should open")
        .write_all(
            json!({ "session_id": "exec-session", "turn_id": "exec-turn" })
                .to_string()
                .as_bytes(),
        )
        .expect("exec hook input should write");
    let output = child.wait_with_output().expect("exec hook should finish");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let task_id = codex_task_id("exec-session", "exec-turn");
    let owner_path = repo
        .join(".stateful_core/runtime/tasks")
        .join(format!("{task_id}.json"));
    let owner: Value =
        serde_json::from_str(&std::fs::read_to_string(&owner_path).expect("owner should read"))
            .expect("owner should decode");
    assert_eq!(owner["pid"], std::process::id());
    let (_, request) = requests
        .recv_timeout(Duration::from_secs(2))
        .expect("task start request should arrive");
    assert!(request.starts_with("POST /v2/tasks/start HTTP/1.1"));
    let deadline = started_at + Duration::from_secs(10);
    let heartbeat_outlived_timeout = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break false;
        }
        match requests.recv_timeout(remaining) {
            Ok((received_at, request))
                if request.starts_with("POST /v2/tasks/heartbeat HTTP/1.1")
                    && received_at.duration_since(started_at) > Duration::from_secs(5) =>
            {
                break true;
            }
            Ok(_) => {}
            Err(_) => break false,
        }
    };
    assert!(
        heartbeat_outlived_timeout,
        "heartbeat helper did not outlive the five-second timeout"
    );
    std::fs::remove_file(owner_path).expect("owner record should remove");
}

#[test]
fn codex_new_turn_finalizes_a_missed_stop_before_creating_its_owner() {
    let temp = tempfile::tempdir().expect("temp directory should create");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).expect("git marker should create");
    std::fs::create_dir_all(repo.join(".stateful")).expect("settings directory should create");
    std::fs::write(
        repo.join(".stateful/config.yml"),
        "heartbeat_interval_seconds: 20\ninactivity_timeout_seconds: 60\n",
    )
    .expect("settings should write");
    let paths = GlobalPaths::new(temp.path().join("stateful"));
    enable_repo(&paths, &repo).expect("repo should enable");
    let (runtime, requests) = runtime_and_live_task_requests();
    write_global_runtime_file(&paths, &runtime).expect("global runtime should write");
    let old_turn = json!({ "session_id": "same-session", "turn_id": "old-turn" }).to_string();
    let new_turn = json!({ "session_id": "same-session", "turn_id": "new-turn" }).to_string();
    let old_task_id = codex_task_id("same-session", "old-turn");
    let new_task_id = codex_task_id("same-session", "new-turn");

    assert!(
        run_codex_hook(&repo, &paths, "user-prompt-submit", &old_turn)
            .status
            .success(),
        "old task start should succeed"
    );
    assert!(
        run_codex_hook(&repo, &paths, "user-prompt-submit", &new_turn)
            .status
            .success(),
        "new task start should succeed"
    );

    let mut observed = Vec::new();
    while !(observed
        .iter()
        .any(|request: &String| request.starts_with("POST /v2/tasks/finalize HTTP/1.1"))
        && observed.iter().any(|request: &String| {
            request.starts_with("POST /v2/tasks/start HTTP/1.1")
                && request_body(request).and_then(|body| body.get("task_id").cloned())
                    == Some(Value::String(new_task_id.clone()))
        }))
    {
        let (_, request) = requests
            .recv_timeout(Duration::from_secs(2))
            .expect("lifecycle request should arrive");
        observed.push(request);
    }
    let finalize_index = observed
        .iter()
        .position(|request| request.starts_with("POST /v2/tasks/finalize HTTP/1.1"))
        .expect("old task should finalize");
    let new_start_index = observed
        .iter()
        .position(|request| {
            request.starts_with("POST /v2/tasks/start HTTP/1.1")
                && request_body(request).and_then(|body| body.get("task_id").cloned())
                    == Some(Value::String(new_task_id.clone()))
        })
        .expect("new task should start");
    assert!(finalize_index < new_start_index);
    let owner_directory = repo.join(".stateful_core/runtime/tasks");
    assert!(!owner_directory.join(format!("{old_task_id}.json")).exists());
    assert!(owner_directory.join(format!("{new_task_id}.json")).exists());
    std::fs::remove_file(owner_directory.join(format!("{new_task_id}.json")))
        .expect("new owner should remove");
}

#[test]
fn codex_new_turn_fails_closed_when_the_missed_stop_is_draining() {
    let temp = tempfile::tempdir().expect("temp directory should create");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).expect("git marker should create");
    let paths = GlobalPaths::new(temp.path().join("stateful"));
    enable_repo(&paths, &repo).expect("repo should enable");
    let old_task_id = codex_task_id("draining-session", "old-turn");
    let owner_directory = repo.join(".stateful_core/runtime/tasks");
    std::fs::create_dir_all(&owner_directory).expect("owner directory should create");
    std::fs::write(
        owner_directory.join(format!("{old_task_id}.json")),
        json!({
            "task_id": old_task_id,
            "agent_id": "codex:draining-session",
            "workspace_id": "workspace",
            "session_id": "draining-session",
            "turn_id": "old-turn",
            "pid": std::process::id(),
            "process_start_identity": stateful_cli::process_start_identity_for_pid(std::process::id())
                .expect("test process identity should resolve"),
            "heartbeat_enabled": true,
            "task_start_observed_at": "2026-08-02T00:00:00Z",
            "task_start_input": {
                "next_action": "Codex root turn active",
                "settings": {
                    "heartbeat_interval_seconds": 1,
                    "inactivity_timeout_seconds": 5,
                    "lease_expiry_seconds": 60,
                    "offer_ttl_seconds": 120
                },
                "expires_at": "2026-08-02T00:00:05Z"
            }
        })
        .to_string(),
    )
    .expect("owner should write");
    let (runtime, _requests) = runtime_and_requests(vec![json!({
        "task_id": "$request_task_id",
        "status": "draining",
        "draining": true,
    })]);
    write_global_runtime_file(&paths, &runtime).expect("global runtime should write");
    let new_task_id = codex_task_id("draining-session", "new-turn");

    let output = run_codex_hook(
        &repo,
        &paths,
        "user-prompt-submit",
        &json!({ "session_id": "draining-session", "turn_id": "new-turn" }).to_string(),
    );
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("draining"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let owner: Value = serde_json::from_str(
        &std::fs::read_to_string(owner_directory.join(format!("{old_task_id}.json")))
            .expect("draining owner should remain"),
    )
    .expect("owner should decode");
    assert_eq!(owner["heartbeat_enabled"], false);
    assert!(!owner_directory.join(format!("{new_task_id}.json")).exists());
    std::fs::remove_file(owner_directory.join(format!("{old_task_id}.json")))
        .expect("owner should remove");
}

#[test]
fn codex_stop_disables_the_heartbeat_when_finalize_fails() {
    let temp = tempfile::tempdir().expect("temp directory should create");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).expect("git marker should create");
    std::fs::create_dir_all(repo.join(".stateful")).expect("settings directory should create");
    std::fs::write(
        repo.join(".stateful/config.yml"),
        "heartbeat_interval_seconds: 20\ninactivity_timeout_seconds: 60\n",
    )
    .expect("settings should write");
    let paths = GlobalPaths::new(temp.path().join("stateful"));
    enable_repo(&paths, &repo).expect("repo should enable");
    let (runtime, _requests) = runtime_and_live_task_requests();
    write_global_runtime_file(&paths, &runtime).expect("global runtime should write");
    let input = json!({ "session_id": "stop-session", "turn_id": "stop-turn" }).to_string();
    assert!(
        run_codex_hook(&repo, &paths, "user-prompt-submit", &input)
            .status
            .success(),
        "task start should succeed"
    );
    write_global_runtime_file(
        &paths,
        &ServerRuntime::new(
            "http://127.0.0.1:9",
            "test-token",
            "workspace",
            std::process::id(),
            stateful_cli::process_start_identity_for_pid(std::process::id())
                .expect("test process identity should resolve"),
        ),
    )
    .expect("unreachable runtime should write");

    let output = run_codex_hook(&repo, &paths, "stop", &input);
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("could not terminally stop"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let owner_path = repo.join(".stateful_core/runtime/tasks").join(format!(
        "{}.json",
        codex_task_id("stop-session", "stop-turn")
    ));
    let owner: Value =
        serde_json::from_str(&std::fs::read_to_string(&owner_path).expect("owner should read"))
            .expect("owner should decode");
    assert_eq!(owner["heartbeat_enabled"], false);
    std::fs::remove_file(owner_path).expect("owner record should remove");
}
