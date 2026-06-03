use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    sync::mpsc,
    thread,
};

use stateful_cli::{
    CurrentSession, GlobalPaths, ServerRuntime, call_mcp_tool_in_repo, handle_mcp_jsonrpc_in_repo,
    serve_mcp_stdio_in_repo, write_current_session_file, write_global_runtime_file,
    write_runtime_file,
};

#[test]
fn mcp_current_read_executes_get_request() {
    let temp_root = temp_root("stateful-mcp-current");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok","current":{}}"#);
    });
    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let response = call_mcp_tool_in_repo(&temp_root, "state.current.read", serde_json::json!({}))
        .expect("mcp current read should execute");

    assert_eq!(response.status_code, 200);
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("GET /v1/current HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_call_discovers_global_runtime_file() {
    let temp_root = temp_root("stateful-mcp-global-runtime");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let repo_root = temp_root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be creatable");
    let paths = GlobalPaths::new(temp_root.join("home"));

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok","current":{}}"#);
    });
    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "global-w", 42);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["mcp", "call", "state.current.read"])
        .current_dir(&repo_root)
        .env_clear()
        .env("STATEFUL_HOME", &paths.home)
        .output()
        .expect("stateful mcp call should run");

    assert!(
        output.status.success(),
        "stateful mcp call failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"status\":\"ok\""));
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("GET /v1/current HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_validation_run_adds_repo_root_and_workspace_id() {
    let temp_root = temp_root("stateful-mcp-validation");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"passed"}"#);
    });
    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let response = call_mcp_tool_in_repo(
        &temp_root,
        "state.validation.run",
        serde_json::json!({"profile": "cargo-test"}),
    )
    .expect("mcp validation run should execute");

    assert_eq!(response.status_code, 200);
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/validation/run HTTP/1.1"));
    assert!(request.contains("\"workspace_id\":\"w1\""));
    assert!(request.contains("\"repo_root\":"));
    assert!(request.contains("\"profile\":\"cargo-test\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_tools_list_returns_stateful_tool_descriptors() {
    let temp_root = temp_root("stateful-mcp-tools-list");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let response = handle_mcp_jsonrpc_in_repo(
        &temp_root,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
    )
    .expect("tools/list should handle")
    .expect("tools/list should produce response");
    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 1);
    let tools = json["result"]["tools"]
        .as_array()
        .expect("tools should be array");
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "state_intent_declare")
    );
    let intent_tool = tools
        .iter()
        .find(|tool| tool["name"] == "state_intent_declare")
        .expect("intent tool should be listed");
    assert_eq!(
        intent_tool["inputSchema"]["required"],
        serde_json::json!(["files_planned"])
    );
    assert_eq!(
        intent_tool["inputSchema"]["properties"]["files_planned"]["items"]["type"],
        "string"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_tools_call_for_intent_declare_posts_to_state_server() {
    let temp_root = temp_root("stateful-mcp-intent-declare");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok"}"#);
    });
    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let response = handle_mcp_jsonrpc_in_repo(
        &temp_root,
        r#"{
          "jsonrpc":"2.0",
          "id":2,
          "method":"tools/call",
          "params":{
            "name":"state_intent_declare",
            "arguments":{
              "session_id":"s1",
              "workspace_id":"w1",
              "files_planned":["src/auth.ts"]
            }
          }
        }"#,
    )
    .expect("tools/call should handle")
    .expect("tools/call should produce response");

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/intent/declare HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    assert!(request.contains("\"session_id\":\"s1\""));
    assert!(request.contains("\"files_planned\":[\"src/auth.ts\"]"));

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 2);
    assert_eq!(json["result"]["isError"], false);
    assert_eq!(json["result"]["content"][0]["type"], "text");
    assert!(
        json["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("\"status\":\"ok\"")
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_intent_declare_defaults_to_current_hook_session() {
    let temp_root = temp_root("stateful-mcp-intent-current-session");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(&mut stream, r#"{"status":"ok"}"#);
    });
    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");
    write_current_session_file(&temp_root, &CurrentSession::new("s-current", "w1"))
        .expect("current session should write");

    let response = handle_mcp_jsonrpc_in_repo(
        &temp_root,
        r#"{
          "jsonrpc":"2.0",
          "id":3,
          "method":"tools/call",
          "params":{
            "name":"state_intent_declare",
            "arguments":{
              "files_planned":["src/auth.ts"]
            }
          }
        }"#,
    )
    .expect("tools/call should handle")
    .expect("tools/call should produce response");

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("\"session_id\":\"s-current\""));
    assert!(request.contains("\"workspace_id\":\"w1\""));
    assert!(request.contains("\"files_planned\":[\"src/auth.ts\"]"));

    let json: serde_json::Value = serde_json::from_str(&response).expect("response should be json");
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 3);
    assert_eq!(json["result"]["isError"], false);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_stdio_accepts_lf_only_content_length_headers() {
    let temp_root = temp_root("stateful-mcp-lf-headers");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    let input = format!("Content-Length: {}\n\n{}", body.len(), body);
    let mut output = Vec::new();

    serve_mcp_stdio_in_repo(&temp_root, input.as_bytes(), &mut output)
        .expect("stdio server should handle LF-only headers");

    let output = String::from_utf8(output).expect("output should be utf8");
    assert!(output.starts_with("Content-Length: "));
    assert!(output.contains("\"jsonrpc\":\"2.0\""));
    assert!(output.contains("\"serverInfo\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn mcp_stdio_accepts_newline_delimited_jsonrpc() {
    let temp_root = temp_root("stateful-mcp-json-line");
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n";
    let mut output = Vec::new();

    serve_mcp_stdio_in_repo(&temp_root, &input[..], &mut output)
        .expect("stdio server should handle newline-delimited JSON-RPC");

    let output = String::from_utf8(output).expect("output should be utf8");
    assert!(output.starts_with('{'));
    assert!(output.ends_with('\n'));
    assert!(output.contains("\"id\":1"));
    assert!(output.contains("\"jsonrpc\":\"2.0\""));
    assert!(output.contains("\"serverInfo\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

fn temp_root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).expect("old temp root should be removable");
    }
    root
}

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
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
        .expect("content length should exist")
        .parse::<usize>()
        .expect("content length should parse");

    let mut body = vec![0_u8; content_length];
    stream
        .read_exact(&mut body)
        .expect("request body should read");
    buffer.extend_from_slice(&body);

    String::from_utf8(buffer).expect("request should be utf8")
}

fn read_http_request_without_body(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut byte = [0_u8; 1];
    while !buffer.ends_with(b"\r\n\r\n") {
        stream
            .read_exact(&mut byte)
            .expect("request header byte should read");
        buffer.push(byte[0]);
    }

    String::from_utf8(buffer).expect("request should be utf8")
}

fn write_json_response(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .expect("response should write");
}
