use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FixtureSource {
    Captured,
    Handwritten,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RuntimeEvent<T> {
    pub source: FixtureSource,
    pub runtime: String,
    pub version: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub cwd: String,
    pub event: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeToolSupport {
    Supported,
    WrapperRequired(&'static str),
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CodexTurnEvent {
    pub session_id: String,
    pub turn_id: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CodexToolCall {
    pub session_id: String,
    pub turn_id: String,
    pub tool_name: String,
    pub tool_use_id: String,
    pub tool_input: Value,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CodexToolResult {
    pub session_id: String,
    pub turn_id: String,
    pub tool_name: String,
    pub tool_use_id: String,
    pub tool_input: Value,
    pub tool_response: Value,
}

impl CodexToolCall {
    pub fn validate_result(&self, result: &CodexToolResult) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.session_id == result.session_id,
            "Codex session_id changed"
        );
        anyhow::ensure!(self.turn_id == result.turn_id, "Codex turn_id changed");
        anyhow::ensure!(
            self.tool_name == result.tool_name,
            "Codex tool_name changed"
        );
        anyhow::ensure!(
            self.tool_use_id == result.tool_use_id,
            "Codex tool_use_id changed"
        );
        anyhow::ensure!(
            self.tool_input == result.tool_input,
            "Codex tool_input changed"
        );
        Ok(())
    }
}

pub fn validate_codex_turn_start_stop(
    start: &CodexTurnEvent,
    stop: &CodexTurnEvent,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        start.session_id == stop.session_id,
        "Codex session_id changed"
    );
    anyhow::ensure!(start.turn_id == stop.turn_id, "Codex turn_id changed");
    Ok(())
}

pub fn codex_native_tool_support(_tool_name: &str) -> NativeToolSupport {
    NativeToolSupport::WrapperRequired(
        "Codex hook payloads do not prove typed terminal status or complete native content; use the Stateful wrapper",
    )
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OmpAgentStart {
    #[serde(rename = "type")]
    pub event_type: String,
    pub session_id: String,
    pub leaf_agent_id: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OmpAgentEnd {
    #[serde(rename = "type")]
    pub event_type: String,
    pub session_id: String,
    pub leaf_agent_id: String,
    #[serde(rename = "task_id", alias = "taskId")]
    pub task_id: String,
    pub will_continue: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OmpSessionShutdown {
    #[serde(rename = "type")]
    pub event_type: String,
    pub session_id: String,
    pub leaf_agent_id: String,
    #[serde(rename = "task_id", alias = "taskId")]
    pub task_id: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OmpToolCall {
    #[serde(rename = "type")]
    pub event_type: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub input: Value,
    #[serde(default, rename = "task_id", alias = "taskId")]
    pub task_id: Option<String>,
    #[serde(default)]
    pub leaf_agent_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OmpToolResult {
    #[serde(rename = "type")]
    pub event_type: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub input: Value,
    pub is_error: bool,
    pub content: Value,
    pub details: Value,
    #[serde(default, rename = "task_id", alias = "taskId")]
    pub task_id: Option<String>,
    #[serde(default)]
    pub leaf_agent_id: Option<String>,
}

impl OmpToolCall {
    pub fn validate_result(&self, result: &OmpToolResult) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.tool_call_id == result.tool_call_id,
            "OMP toolCallId changed"
        );
        anyhow::ensure!(self.tool_name == result.tool_name, "OMP toolName changed");
        anyhow::ensure!(self.input == result.input, "OMP input changed");
        Ok(())
    }
}

pub fn validate_omp_agent_start(event: &RuntimeEvent<OmpAgentStart>) -> anyhow::Result<()> {
    validate_omp_event(event, "agent_start", &event.event.event_type)?;
    anyhow::ensure!(
        !event.event.session_id.trim().is_empty(),
        "OMP agent_start lacks sessionId"
    );
    anyhow::ensure!(
        !event.event.leaf_agent_id.trim().is_empty(),
        "OMP agent_start lacks leafAgentId"
    );
    Ok(())
}

pub fn validate_omp_agent_end(event: &RuntimeEvent<OmpAgentEnd>) -> anyhow::Result<()> {
    validate_omp_event(event, "agent_end", &event.event.event_type)?;
    anyhow::ensure!(
        !event.event.session_id.trim().is_empty(),
        "OMP agent_end lacks sessionId"
    );
    anyhow::ensure!(
        !event.event.leaf_agent_id.trim().is_empty(),
        "OMP agent_end lacks leafAgentId"
    );
    anyhow::ensure!(
        !event.event.task_id.trim().is_empty(),
        "OMP agent_end lacks task_id"
    );
    Ok(())
}

pub fn validate_omp_session_shutdown(
    event: &RuntimeEvent<OmpSessionShutdown>,
) -> anyhow::Result<()> {
    validate_omp_event(event, "session_shutdown", &event.event.event_type)?;
    anyhow::ensure!(
        !event.event.session_id.trim().is_empty(),
        "OMP session_shutdown lacks sessionId"
    );
    anyhow::ensure!(
        !event.event.leaf_agent_id.trim().is_empty(),
        "OMP session_shutdown lacks leafAgentId"
    );
    anyhow::ensure!(
        !event.event.task_id.trim().is_empty(),
        "OMP session_shutdown lacks task_id"
    );
    Ok(())
}

pub fn omp_native_read_support(
    call: &RuntimeEvent<OmpToolCall>,
    result: &RuntimeEvent<OmpToolResult>,
) -> NativeToolSupport {
    if !is_same_captured_omp_version(call, result) {
        return NativeToolSupport::WrapperRequired(
            "OMP payload was not captured from one runtime version",
        );
    }
    if call.event_type != "tool_call" || result.event_type != "tool_result" {
        return NativeToolSupport::WrapperRequired("OMP tool event type is invalid");
    }
    if call.event.validate_result(&result.event).is_err() {
        return NativeToolSupport::WrapperRequired("OMP tool_call/tool_result correlation failed");
    }
    if call.event.event_type != "tool_call" || result.event.event_type != "tool_result" {
        return NativeToolSupport::WrapperRequired("OMP nested tool event type is invalid");
    }
    if call.event.tool_name != "read" || result.event.is_error {
        return NativeToolSupport::WrapperRequired("OMP result is not a successful native read");
    }
    let Some(path) = call.event.input.get("path").and_then(Value::as_str) else {
        return NativeToolSupport::WrapperRequired("OMP read path is missing");
    };
    if !path.ends_with(":raw")
        || call.event.input.get("offset").is_some()
        || call.event.input.get("limit").is_some()
    {
        return NativeToolSupport::WrapperRequired("OMP read is not an unrestricted :raw read");
    }
    let Some(source) = result
        .event
        .details
        .pointer("/meta/source")
        .filter(|value| value.get("type").and_then(Value::as_str) == Some("path"))
        .and_then(|value| value.get("value").and_then(Value::as_str))
    else {
        return NativeToolSupport::WrapperRequired("OMP read result lacks path source metadata");
    };
    if source.is_empty()
        || non_null(result.event.details.get("truncation"))
        || non_null(result.event.details.pointer("/meta/truncation"))
        || non_null(result.event.details.pointer("/meta/limits"))
    {
        return NativeToolSupport::WrapperRequired(
            "OMP read result is truncated or not path-backed",
        );
    }
    let Some(content) = result.event.content.as_array() else {
        return NativeToolSupport::WrapperRequired("OMP read content is not an array");
    };
    let [entry] = content.as_slice() else {
        return NativeToolSupport::WrapperRequired("OMP read result is not one exact text payload");
    };
    let Some(text) = entry
        .get("text")
        .filter(|_| entry.get("type").and_then(Value::as_str) == Some("text"))
        .and_then(Value::as_str)
    else {
        return NativeToolSupport::WrapperRequired("OMP read result is not one exact text payload");
    };
    if result.event.details.get("fileSize").and_then(Value::as_u64) != Some(text.len() as u64)
        || result
            .event
            .details
            .pointer("/displayContent/text")
            .and_then(Value::as_str)
            != Some(text)
    {
        return NativeToolSupport::WrapperRequired(
            "OMP read output differs from its file size or display payload",
        );
    }
    NativeToolSupport::Supported
}

pub fn omp_native_write_support(
    call: &RuntimeEvent<OmpToolCall>,
    result: &RuntimeEvent<OmpToolResult>,
) -> NativeToolSupport {
    if !is_same_captured_omp_version(call, result) {
        return NativeToolSupport::WrapperRequired(
            "OMP payload was not captured from one runtime version",
        );
    }
    if call.event_type != "tool_call"
        || result.event_type != "tool_result"
        || call.event.event_type != "tool_call"
        || result.event.event_type != "tool_result"
    {
        return NativeToolSupport::WrapperRequired("OMP tool event type is invalid");
    }
    if call.event.validate_result(&result.event).is_err() {
        return NativeToolSupport::WrapperRequired("OMP tool_call/tool_result correlation failed");
    }
    if call.event.tool_name != "write" || result.event.is_error {
        return NativeToolSupport::WrapperRequired("OMP result is not a successful native write");
    }
    if result
        .event
        .details
        .get("resolvedPath")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return NativeToolSupport::WrapperRequired("OMP write result lacks resolvedPath");
    }
    NativeToolSupport::Supported
}

fn non_null(value: Option<&Value>) -> bool {
    value.is_some_and(|value| !value.is_null())
}

fn validate_omp_event<T>(
    event: &RuntimeEvent<T>,
    expected_outer: &str,
    expected_inner: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(event.runtime == "omp", "runtime is not OMP");
    anyhow::ensure!(event.version == "17.2.3", "OMP version is not 17.2.3");
    anyhow::ensure!(
        event.event_type == expected_outer,
        "OMP outer event type changed"
    );
    anyhow::ensure!(
        expected_inner == expected_outer,
        "OMP inner event type changed"
    );
    Ok(())
}

fn is_same_captured_omp_version<T, U>(left: &RuntimeEvent<T>, right: &RuntimeEvent<U>) -> bool {
    left.source == FixtureSource::Captured
        && right.source == FixtureSource::Captured
        && left.runtime == "omp"
        && right.runtime == "omp"
        && left.version == "17.2.3"
        && right.version == "17.2.3"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Fixtures {
        codex: CodexFixtures,
        omp: OmpFixtures,
    }

    #[derive(Deserialize)]
    struct CodexFixtures {
        source: FixtureSource,
        user_prompt_submit: CodexTurnEvent,
        stop: CodexTurnEvent,
        pre_read: CodexToolCall,
        post_read: CodexToolResult,
        pre_write: CodexToolCall,
        post_write: CodexToolResult,
    }

    #[derive(Deserialize)]
    struct OmpFixtures {
        tool_call_read: RuntimeEvent<OmpToolCall>,
        tool_result_read: RuntimeEvent<OmpToolResult>,
        tool_call_write: RuntimeEvent<OmpToolCall>,
        tool_result_write: RuntimeEvent<OmpToolResult>,
        lifecycle: OmpLifecycleFixtures,
        forwarded_tool_result: RuntimeEvent<OmpToolResult>,
    }

    #[derive(Deserialize)]
    struct OmpLifecycleFixtures {
        source: FixtureSource,
        agent_start: RuntimeEvent<OmpAgentStart>,
        agent_end_continue: RuntimeEvent<OmpAgentEnd>,
        agent_end_finalize: RuntimeEvent<OmpAgentEnd>,
        session_shutdown: RuntimeEvent<OmpSessionShutdown>,
    }

    fn fixtures() -> Fixtures {
        serde_json::from_str(include_str!("../tests/fixtures/runtime-contract.json"))
            .expect("runtime contract fixtures should parse")
    }

    #[test]
    fn codex_turn_is_deterministic_but_native_tools_require_wrapper() {
        let codex = fixtures().codex;
        validate_codex_turn_start_stop(&codex.user_prompt_submit, &codex.stop)
            .expect("Codex start and stop should identify one turn");
        codex
            .pre_read
            .validate_result(&codex.post_read)
            .expect("Codex read invocation should correlate");
        codex
            .pre_write
            .validate_result(&codex.post_write)
            .expect("Codex write invocation should correlate");
        assert_eq!(codex.source, FixtureSource::Handwritten);
        assert!(matches!(
            codex_native_tool_support("read"),
            NativeToolSupport::WrapperRequired(_)
        ));
        assert!(matches!(
            codex_native_tool_support("apply_patch"),
            NativeToolSupport::WrapperRequired(_)
        ));
    }

    #[test]
    fn captured_omp_17_2_3_fixture_proves_typed_native_read_and_write_results() {
        let omp = fixtures().omp;
        assert_eq!(
            omp_native_read_support(&omp.tool_call_read, &omp.tool_result_read),
            NativeToolSupport::Supported
        );
        assert_eq!(
            omp_native_write_support(&omp.tool_call_write, &omp.tool_result_write),
            NativeToolSupport::Supported
        );
    }

    #[test]
    fn omp_lifecycle_fixture_requires_owner_and_will_continue() {
        let lifecycle = fixtures().omp.lifecycle;
        assert_eq!(lifecycle.source, FixtureSource::Handwritten);
        validate_omp_agent_start(&lifecycle.agent_start)
            .expect("agent_start owner should validate");
        validate_omp_agent_end(&lifecycle.agent_end_continue)
            .expect("continuing agent_end should validate");
        validate_omp_agent_end(&lifecycle.agent_end_finalize)
            .expect("final agent_end should validate");
        validate_omp_session_shutdown(&lifecycle.session_shutdown)
            .expect("session_shutdown should validate");
        assert!(lifecycle.agent_end_continue.event.will_continue);
        assert!(!lifecycle.agent_end_finalize.event.will_continue);
        assert_eq!(
            lifecycle.agent_start.event.leaf_agent_id,
            lifecycle.agent_end_finalize.event.leaf_agent_id
        );
        assert_eq!(
            lifecycle.agent_start.event.leaf_agent_id,
            lifecycle.session_shutdown.event.leaf_agent_id
        );
        assert_eq!(
            lifecycle.agent_end_finalize.event.task_id,
            lifecycle.session_shutdown.event.task_id
        );
    }

    #[test]
    fn extension_forwarding_preserves_terminal_payload_and_task_owner() {
        let result = fixtures().omp.forwarded_tool_result;
        assert_eq!(result.event.task_id.as_deref(), Some("omp-task-root"));
        assert_eq!(result.event.leaf_agent_id.as_deref(), Some("root"));
        assert!(!result.event.is_error);
        assert!(result.event.content.get(0).is_some());
        assert!(result.event.details.get("resolvedPath").is_some());
    }

    #[test]
    fn incomplete_omp_read_cannot_become_authoritative() {
        let omp = fixtures().omp;
        let mut wrong_size = omp.tool_result_read.clone();
        wrong_size.event.details["fileSize"] = Value::from(30_u64);
        assert!(matches!(
            omp_native_read_support(&omp.tool_call_read, &wrong_size),
            NativeToolSupport::WrapperRequired(_)
        ));

        let mut truncated = omp.tool_result_read;
        truncated.event.details["truncation"] = serde_json::json!({ "truncated": true });
        assert!(matches!(
            omp_native_read_support(&omp.tool_call_read, &truncated),
            NativeToolSupport::WrapperRequired(_)
        ));
    }
}
