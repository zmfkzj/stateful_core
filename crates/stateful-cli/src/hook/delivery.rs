use std::io::Write;

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

#[derive(Debug)]
pub(crate) struct RenderedContext {
    pub(crate) prompt_text: String,
    acknowledgement: Option<ContextAcknowledgement>,
}

#[derive(Debug)]
struct ContextAcknowledgement {
    delivery_id: String,
    sequence: u64,
    workspace_version: u64,
}

pub(crate) fn render_context(
    runtime: &ServerRuntime,
    agent_id: &str,
    workspace_id: &str,
    identity: Option<&RepoIdentity>,
    event: &str,
    source_ref: &str,
) -> anyhow::Result<RenderedContext> {
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
    let acknowledgement = if delta.changed {
        let (Some(delivery_id), Some(sequence)) = (delta.delivery_id, delta.sequence) else {
            anyhow::bail!("changed context delivery did not include acknowledgement metadata");
        };
        Some(ContextAcknowledgement {
            delivery_id,
            sequence,
            workspace_version: delta.workspace_version,
        })
    } else {
        None
    };
    Ok(RenderedContext {
        prompt_text: delta.prompt_text,
        acknowledgement,
    })
}

pub(crate) fn acknowledge_context(
    runtime: &ServerRuntime,
    agent_id: &str,
    workspace_id: &str,
    identity: Option<&RepoIdentity>,
    source_ref: &str,
    context: &RenderedContext,
) -> anyhow::Result<()> {
    let Some(acknowledgement) = context.acknowledgement.as_ref() else {
        return Ok(());
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
            "delivery_id": acknowledgement.delivery_id,
            "sequence": acknowledgement.sequence,
            "workspace_version": acknowledgement.workspace_version,
        }),
    )?;
    post_v2(runtime, "/v2/context/ack", &acknowledgement)?;
    Ok(())
}

pub(crate) fn write_and_ack_context<W, A>(
    writer: &mut W,
    context: &RenderedContext,
    acknowledge: A,
) -> anyhow::Result<()>
where
    W: Write + ?Sized,
    A: FnOnce() -> anyhow::Result<()>,
{
    if !context.prompt_text.is_empty() {
        writer.write_all(context.prompt_text.as_bytes())?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    if context.acknowledgement.is_some() {
        acknowledge()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        io::{self, Write},
        rc::Rc,
    };

    use super::*;

    struct BrokenPipeWriter;

    impl Write for BrokenPipeWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct TrackingWriter {
        state: Rc<RefCell<(Vec<u8>, bool)>>,
    }

    impl Write for TrackingWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.state.borrow_mut().0.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.state.borrow_mut().1 = true;
            Ok(())
        }
    }

    #[test]
    fn write_failure_leaves_changed_context_unacknowledged() {
        let context = RenderedContext {
            prompt_text: "context".to_string(),
            acknowledgement: Some(ContextAcknowledgement {
                delivery_id: "delivery-1".to_string(),
                sequence: 1,
                workspace_version: 1,
            }),
        };
        let mut writer = BrokenPipeWriter;
        let mut acknowledgements = 0;

        let result = write_and_ack_context(&mut writer, &context, || {
            acknowledgements += 1;
            Ok(())
        });

        assert!(result.is_err());
        assert_eq!(acknowledgements, 0);
    }

    #[test]
    fn acknowledgement_follows_output_and_flush() {
        let context = RenderedContext {
            prompt_text: "context".to_string(),
            acknowledgement: Some(ContextAcknowledgement {
                delivery_id: "delivery-1".to_string(),
                sequence: 1,
                workspace_version: 1,
            }),
        };
        let state = Rc::new(RefCell::new((Vec::new(), false)));
        let mut writer = TrackingWriter {
            state: Rc::clone(&state),
        };
        let mut acknowledgements = 0;

        write_and_ack_context(&mut writer, &context, || {
            acknowledgements += 1;
            assert!(state.borrow().1);
            Ok(())
        })
        .expect("delivery should be acknowledged after its output is flushed");

        assert_eq!(state.borrow().0, b"context\n");
        assert_eq!(acknowledgements, 1);
    }
    #[test]
    fn retrying_the_same_delivery_acknowledges_only_after_a_successful_write() {
        let context = RenderedContext {
            prompt_text: "context".to_string(),
            acknowledgement: Some(ContextAcknowledgement {
                delivery_id: "delivery-1".to_string(),
                sequence: 1,
                workspace_version: 1,
            }),
        };
        let mut acknowledgements = 0;
        let mut failed_writer = BrokenPipeWriter;

        assert!(
            write_and_ack_context(&mut failed_writer, &context, || {
                acknowledgements += 1;
                Ok(())
            })
            .is_err()
        );
        let state = Rc::new(RefCell::new((Vec::new(), false)));
        let mut successful_writer = TrackingWriter {
            state: Rc::clone(&state),
        };
        write_and_ack_context(&mut successful_writer, &context, || {
            acknowledgements += 1;
            Ok(())
        })
        .expect("retry should acknowledge the same delivery after writing it");

        assert_eq!(state.borrow().0, b"context\n");
        assert_eq!(acknowledgements, 1);
    }

    #[test]
    fn empty_changed_context_is_acknowledged_after_no_content_flush() {
        let context = RenderedContext {
            prompt_text: String::new(),
            acknowledgement: Some(ContextAcknowledgement {
                delivery_id: "delivery-1".to_string(),
                sequence: 1,
                workspace_version: 1,
            }),
        };
        let state = Rc::new(RefCell::new((Vec::new(), false)));
        let mut writer = TrackingWriter {
            state: Rc::clone(&state),
        };
        let mut acknowledgements = 0;

        write_and_ack_context(&mut writer, &context, || {
            acknowledgements += 1;
            assert!(state.borrow().1);
            Ok(())
        })
        .expect("empty delivery should be acknowledged after its no-content flush");

        assert!(state.borrow().0.is_empty());
        assert_eq!(acknowledgements, 1);
    }
}
