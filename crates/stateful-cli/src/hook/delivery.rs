use serde::{Deserialize, Serialize};
use serde_json::json;
use stateful_core::{ActorType, SourceKind};

use crate::{
    RepoIdentity, ServerRuntime, get_v2, post_v2, v2_query_for_runtime, v2_request_envelope,
};

#[derive(Debug, Deserialize)]
struct ContextDelta {
    changed: bool,
    #[serde(default)]
    delivery_id: Option<String>,
    #[serde(default)]
    sequence: Option<u64>,
    workspace_version: u64,
    prompt_text: String,
}

#[derive(Debug, Deserialize)]
struct CurrentResponse {
    presence: Option<CurrentPresence>,
}

#[derive(Debug, Deserialize)]
struct CurrentPresence {
    goal_excerpt: Option<String>,
}

#[derive(Serialize)]
struct CurrentQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    resource: Option<String>,
}

pub(crate) fn presence_has_goal(
    runtime: &ServerRuntime,
    agent_id: &str,
    workspace_id: &str,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<bool> {
    let query = v2_query_for_runtime(
        uuid::Uuid::new_v4(),
        agent_id.to_string(),
        workspace_id.to_string(),
        identity.cloned(),
        SourceKind::Hook,
        "codex_prompt_lookup",
        "hook:codex_user_prompt",
        None,
        CurrentQuery { resource: None },
    )?;
    let response = get_v2(runtime, "/v2/current", &query)?;
    let current: CurrentResponse = serde_json::from_str(&response.body)?;
    Ok(current
        .presence
        .and_then(|presence| presence.goal_excerpt)
        .is_some_and(|goal| !goal.trim().is_empty()))
}

pub(crate) fn render_and_ack_context(
    runtime: &ServerRuntime,
    agent_id: &str,
    workspace_id: &str,
    identity: Option<&RepoIdentity>,
    event: &str,
    source_ref: &str,
) -> anyhow::Result<String> {
    let render = v2_request_envelope(
        uuid::Uuid::new_v4(),
        agent_id.to_string(),
        workspace_id.to_string(),
        identity.cloned(),
        ActorType::Agent,
        SourceKind::Hook,
        event,
        source_ref,
        None,
        json!({ "mode": "brief" }),
    )?;
    let response = post_v2(runtime, "/v2/context/render", &render)?;
    let delta: ContextDelta = serde_json::from_str(&response.body)?;
    if delta.changed {
        let (Some(delivery_id), Some(sequence)) = (delta.delivery_id, delta.sequence) else {
            anyhow::bail!("changed context delivery did not include acknowledgement metadata");
        };
        let acknowledgement = v2_request_envelope(
            uuid::Uuid::new_v4(),
            agent_id.to_string(),
            workspace_id.to_string(),
            identity.cloned(),
            ActorType::Agent,
            SourceKind::Hook,
            "context_ack",
            source_ref,
            None,
            json!({
                "delivery_id": delivery_id,
                "sequence": sequence,
                "workspace_version": delta.workspace_version,
            }),
        )?;
        post_v2(runtime, "/v2/context/ack", &acknowledgement)?;
    }
    Ok(delta.prompt_text)
}
