use crate::{AgentIdentity, ProtocolVersion, SourceRef, WorkspaceIdentity};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{error::Error, fmt};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct V2Error {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_next_action: Option<String>,
}

impl V2Error {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            required_next_action: None,
        }
    }

    pub fn with_required_next_action(mut self, action: impl Into<String>) -> Self {
        self.required_next_action = Some(action.into());
        self
    }

    pub fn envelope(self, request_id: Uuid) -> V2ErrorEnvelope {
        V2ErrorEnvelope {
            protocol_version: ProtocolVersion::V2,
            request_id,
            error: self,
        }
    }
}

impl fmt::Display for V2Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl Error for V2Error {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct V2ErrorEnvelope {
    pub protocol_version: ProtocolVersion,
    pub request_id: Uuid,
    pub error: V2Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestEnvelope<T> {
    pub protocol_version: ProtocolVersion,
    pub request_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub observed_at: OffsetDateTime,
    pub agent: AgentIdentity,
    pub workspace: WorkspaceIdentity,
    pub source: SourceRef,
    pub payload: T,
}

impl<T> RequestEnvelope<T> {
    pub fn new(
        request_id: Uuid,
        observed_at: OffsetDateTime,
        agent: AgentIdentity,
        workspace: WorkspaceIdentity,
        source: SourceRef,
        payload: T,
    ) -> Result<Self, V2Error> {
        validate_request_id(request_id)?;
        agent.validate()?;
        workspace.validate()?;
        source.validate()?;
        Ok(Self {
            protocol_version: ProtocolVersion::V2,
            request_id,
            observed_at,
            agent,
            workspace,
            source,
            payload,
        })
    }

    pub fn validate(&self) -> Result<(), V2Error> {
        validate_request_id(self.request_id)?;
        self.agent.validate()?;
        self.workspace.validate()?;
        self.source.validate()
    }
}

impl<T: DeserializeOwned> RequestEnvelope<T> {
    pub fn from_json(input: impl AsRef<str>) -> Result<Self, V2Error> {
        let value: serde_json::Value = serde_json::from_str(input.as_ref()).map_err(|error| {
            V2Error::new("invalid_request_envelope", error.to_string())
        })?;
        match value.get("protocol_version").and_then(serde_json::Value::as_str) {
            Some("stateful.v2") => {}
            Some(_) => {
                return Err(V2Error::new(
                    "unsupported_protocol",
                    "Only stateful.v2 protocol requests are supported.",
                ));
            }
            None => {
                return Err(V2Error::new(
                    "missing_protocol_version",
                    "A stateful.v2 protocol_version is required.",
                ));
            }
        }
        let envelope: Self = serde_json::from_value(value)
            .map_err(|error| V2Error::new("invalid_request_envelope", error.to_string()))?;
        envelope.validate()?;
        Ok(envelope)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryEnvelope<Q> {
    pub protocol_version: ProtocolVersion,
    pub request_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub observed_at: OffsetDateTime,
    #[serde(flatten)]
    pub agent: AgentIdentity,
    #[serde(flatten)]
    pub workspace: WorkspaceIdentity,
    #[serde(flatten)]
    pub source: SourceRef,
    #[serde(flatten)]
    pub query: Q,
}

impl<Q> QueryEnvelope<Q> {
    pub fn new(
        request_id: Uuid,
        observed_at: OffsetDateTime,
        agent: AgentIdentity,
        workspace: WorkspaceIdentity,
        source: SourceRef,
        query: Q,
    ) -> Result<Self, V2Error> {
        validate_request_id(request_id)?;
        agent.validate()?;
        workspace.validate()?;
        source.validate()?;
        Ok(Self {
            protocol_version: ProtocolVersion::V2,
            request_id,
            observed_at,
            agent,
            workspace,
            source,
            query,
        })
    }

    pub fn validate(&self) -> Result<(), V2Error> {
        validate_request_id(self.request_id)?;
        self.agent.validate()?;
        self.workspace.validate()?;
        self.source.validate()
    }
}

fn validate_request_id(request_id: Uuid) -> Result<(), V2Error> {
    if request_id.is_nil() {
        return Err(V2Error::new(
            "invalid_request_id",
            "request_id must be a non-nil UUID.",
        ));
    }
    Ok(())
}
