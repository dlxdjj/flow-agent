use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_HOOK_PAYLOAD_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
    Gemini,
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Claude => f.write_str("claude"),
            Self::Codex => f.write_str("codex"),
            Self::Gemini => f.write_str("gemini"),
        }
    }
}

impl FromStr for Provider {
    type Err = ParseProviderError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "gemini" => Ok(Self::Gemini),
            _ => Err(ParseProviderError(value.to_owned())),
        }
    }
}

#[derive(Debug, Error)]
#[error("unsupported provider: {0}")]
pub struct ParseProviderError(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeRequest {
    pub v: u16,
    pub id: Uuid,
    pub provider: Provider,
    pub received_at: u64,
    pub needs_reply: bool,
    pub raw: Value,
}

impl BridgeRequest {
    pub fn from_hook(provider: Provider, raw: Value) -> Self {
        let needs_reply = raw
            .get("hook_event_name")
            .and_then(Value::as_str)
            .is_some_and(|name| name == "PermissionRequest");

        Self {
            v: PROTOCOL_VERSION,
            id: Uuid::now_v7(),
            provider,
            received_at: now_millis(),
            needs_reply,
            raw,
        }
    }

    pub fn event_name(&self) -> Option<&str> {
        self.raw.get("hook_event_name").and_then(Value::as_str)
    }

    pub fn session_id(&self) -> Option<&str> {
        self.raw.get("session_id").and_then(Value::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeResponse {
    pub request_id: Uuid,
    pub decision: Option<Decision>,
}

impl BridgeResponse {
    pub fn acknowledged(request_id: Uuid) -> Self {
        Self {
            request_id,
            decision: None,
        }
    }

    pub fn decided(request_id: Uuid, decision: Decision) -> Self {
        Self {
            request_id,
            decision: Some(decision),
        }
    }
}

pub fn permission_directive(provider: Provider, decision: Decision) -> Option<Value> {
    if provider == Provider::Gemini {
        return None;
    }

    let behavior = match decision {
        Decision::Allow => "allow",
        Decision::Deny => "deny",
    };
    let mut decision_value = serde_json::json!({ "behavior": behavior });
    if decision == Decision::Deny {
        decision_value["message"] = Value::String("User denied the permission request".into());
    }

    Some(serde_json::json!({
        "continue": true,
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": decision_value
        }
    }))
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_only_permission_requests_as_blocking() {
        let permission = BridgeRequest::from_hook(
            Provider::Claude,
            serde_json::json!({"hook_event_name": "PermissionRequest"}),
        );
        let stop = BridgeRequest::from_hook(
            Provider::Claude,
            serde_json::json!({"hook_event_name": "Stop"}),
        );

        assert!(permission.needs_reply);
        assert!(!stop.needs_reply);
    }

    #[test]
    fn encodes_provider_permission_directive() {
        let value = permission_directive(Provider::Codex, Decision::Deny).unwrap();
        assert_eq!(
            value.pointer("/hookSpecificOutput/decision/behavior"),
            Some(&Value::String("deny".into()))
        );
    }

    #[test]
    fn gemini_has_no_v1_permission_directive() {
        assert_eq!(
            permission_directive(Provider::Gemini, Decision::Allow),
            None
        );
    }
}
