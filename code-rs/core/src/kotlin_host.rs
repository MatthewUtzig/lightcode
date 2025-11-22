use anyhow::{anyhow, Result};
use code_kotlin_host as host;
use code_protocol::models::{ContentItem, ResponseItem};
use serde::{de::Error as DeError, Deserialize, Deserializer};
use serde_json::{json, to_value, Value};

#[derive(Debug)]
pub struct KotlinCoreHost {
    session_id: String,
    next_cursor: u64,
}

#[derive(Deserialize)]
struct StartSessionResponse {
    status: String,
    #[serde(rename = "session_id", deserialize_with = "deserialize_session_id")]
    session_id: String,
}

#[derive(Deserialize)]
struct SimpleStatus {
    status: String,
}

#[derive(Deserialize)]
struct PollResponse {
    status: String,
    events: Vec<EngineEventRaw>,
    #[serde(rename = "next_cursor")]
    next_cursor: u64,
}

#[derive(Deserialize, Clone)]
#[allow(dead_code)]
pub struct EngineEvent {
    pub seq: u64,
    pub kind: String,
    pub payload: serde_json::Value,
}

#[derive(Deserialize, Clone)]
#[allow(dead_code)]
struct EngineEventRaw {
    pub seq: u64,
    pub kind: String,
    pub payload: serde_json::Value,
}

fn deserialize_session_id<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::String(id) => Ok(id),
        Value::Number(num) => Ok(num.to_string()),
        other => Err(DeError::custom(format!(
            "expected string or number for session_id, got {other:?}"
        ))),
    }
}

impl KotlinCoreHost {
    pub fn new() -> Result<Self> {
        let raw = host::start_session("{}")?;
        let parsed: StartSessionResponse = serde_json::from_str(&raw)?;
        if parsed.status != "ok" {
            return Err(anyhow!("failed to start Kotlin session"));
        }
        Ok(Self {
            session_id: parsed.session_id,
            next_cursor: 0,
        })
    }

    pub fn submit_json(&self, payload: &serde_json::Value) -> Result<()> {
        let resp: SimpleStatus = serde_json::from_str(&host::submit_turn(&self.session_id, &payload.to_string())?)?;
        if resp.status == "ok" {
            Ok(())
        } else {
            Err(anyhow!("failed to submit Kotlin turn"))
        }
    }

    pub fn poll_events(&mut self) -> Result<Vec<EngineEvent>> {
        let cursor_payload = json!({"cursor": self.next_cursor});
        let raw = host::poll_events(&self.session_id, &cursor_payload.to_string())?;
        let parsed: PollResponse = serde_json::from_str(&raw)?;
        if parsed.status != "ok" {
            return Err(anyhow!("poll failed"));
        }
        self.next_cursor = parsed.next_cursor;
        Ok(parsed
            .events
            .into_iter()
            .map(|event| EngineEvent {
                seq: event.seq,
                kind: event.kind,
                payload: event.payload,
            })
            .collect())
    }
}

impl Drop for KotlinCoreHost {
    fn drop(&mut self) {
        let _ = host::close_session(&self.session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_turn_payload_includes_history() {
        let item = ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "hello from test".to_string(),
            }],
        };
        let payload = chat_turn_payload(&[item.clone()], &[item.clone()]).expect("payload");
        assert_eq!(payload.get("type").and_then(|v| v.as_str()), Some("chat_turn"));
        assert_eq!(payload.get("history").and_then(|v| v.as_array()).map(|a| a.len()), Some(1));
        assert_eq!(payload.get("turn_input").and_then(|v| v.as_array()).map(|a| a.len()), Some(1));
    }
}

pub fn chat_turn_payload(history: &[ResponseItem], turn_input: &[ResponseItem]) -> Result<Value> {
    fn serialize(items: &[ResponseItem]) -> Result<Vec<Value>> {
        items
            .iter()
            .map(|item| to_value(item).map_err(|err| anyhow!("failed to serialize response item: {err}")))
            .collect()
    }

    Ok(json!({
        "type": "chat_turn",
        "history": serialize(history)?,
        "turn_input": serialize(turn_input)?,
    }))
}

pub fn run_kotlin_turn(prompt: &str) -> Result<Vec<String>> {
    let host = KotlinCoreHost::new()?;
    let user_item = ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: prompt.to_string(),
        }],
    };
    let submission = chat_turn_payload(&[], &[user_item])?;
    host.submit_json(&submission)?;
    let mut host = host;
    let events = host.poll_events()?;
    let mut messages = Vec::new();
    for event in events {
        if event.kind == "agent_message" {
            if let Some(text) = event.payload.get("message").and_then(|v| v.as_str()) {
                messages.push(text.to_string());
            }
        }
    }
    Ok(messages)
}

/// Lightweight configuration handle for the experimental Kotlin coordinator
/// runtime. At the moment it only stores optional session configuration JSON,
/// but it gives the auto-drive layer a stable type to request when it needs to
/// opt in to the Kotlin-backed coordinator loop.
#[derive(Clone, Default)]
pub struct KotlinAutoCoordinatorRuntime {
    session_config: Option<Value>,
}

impl KotlinAutoCoordinatorRuntime {
    pub fn new() -> Self {
        Self { session_config: None }
    }

    pub fn with_session_config(session_config: Value) -> Self {
        Self {
            session_config: Some(session_config),
        }
    }

    pub fn session_config(&self) -> Option<&Value> {
        self.session_config.as_ref()
    }
}
