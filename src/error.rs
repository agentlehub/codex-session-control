use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ControllerError {
    #[error("{0}")]
    Operational(String),
    #[error("invalid {field}: {reason}")]
    InvalidData {
        field: &'static str,
        reason: &'static str,
    },
}

impl ControllerError {
    pub const fn exit_code(&self) -> u8 {
        1
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorCategory {
    InvalidRequest,
    PolicyRejected,
    TargetUnavailable,
    AuthorityTransportFailure,
    StageTimeout,
    NativeConflict,
    NativeError,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeErrorSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<i64>,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolErrorData {
    pub category: ToolErrorCategory,
    pub message: String,
    pub tool: String,
    pub stage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeErrorSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<DispatchState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconciliation_error: Option<String>,
}

impl ToolErrorData {
    pub fn fixed(category: ToolErrorCategory, tool: &str, stage: &str) -> Self {
        Self {
            category,
            message: category.default_message().to_owned(),
            tool: tool.to_owned(),
            stage: stage.to_owned(),
            thread_id: None,
            turn_id: None,
            native: None,
            dispatch: None,
            observation: None,
            reconciliation_error: None,
        }
    }
}

impl ToolErrorCategory {
    const fn default_message(self) -> &'static str {
        match self {
            Self::InvalidRequest => "request validation failed",
            Self::PolicyRejected => "request rejected by session-control policy",
            Self::TargetUnavailable => "target is unavailable",
            Self::AuthorityTransportFailure => "app-server transport failed",
            Self::StageTimeout => "native stage timed out",
            Self::NativeConflict => "native state conflicts with the request",
            Self::NativeError => "native app-server request failed",
            Self::OutcomeUnknown => {
                "Mutation outcome is unknown. The request may already have been applied."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchState {
    NotDispatched,
    MayHaveBeenDispatched,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ERROR_CATEGORIES: [(&str, ToolErrorCategory); 8] = [
        ("invalid_request", ToolErrorCategory::InvalidRequest),
        ("policy_rejected", ToolErrorCategory::PolicyRejected),
        ("target_unavailable", ToolErrorCategory::TargetUnavailable),
        (
            "authority_transport_failure",
            ToolErrorCategory::AuthorityTransportFailure,
        ),
        ("stage_timeout", ToolErrorCategory::StageTimeout),
        ("native_conflict", ToolErrorCategory::NativeConflict),
        ("native_error", ToolErrorCategory::NativeError),
        ("outcome_unknown", ToolErrorCategory::OutcomeUnknown),
    ];

    #[test]
    fn error_categories_use_exact_public_names() {
        for (expected, category) in ERROR_CATEGORIES {
            assert_eq!(serde_json::to_value(category).unwrap(), json!(expected));
        }

        for (expected, dispatch) in [
            ("not_dispatched", DispatchState::NotDispatched),
            (
                "may_have_been_dispatched",
                DispatchState::MayHaveBeenDispatched,
            ),
        ] {
            assert_eq!(serde_json::to_value(dispatch).unwrap(), json!(expected));
        }
    }

    #[test]
    fn optional_error_context_is_omitted_when_absent() {
        let error = ToolErrorData {
            category: ToolErrorCategory::StageTimeout,
            message: "native stage timed out".to_owned(),
            tool: "thread_read".to_owned(),
            stage: "thread/read".to_owned(),
            thread_id: None,
            turn_id: None,
            native: None,
            dispatch: None,
            observation: None,
            reconciliation_error: None,
        };
        assert_eq!(
            serde_json::to_value(error).unwrap(),
            json!({
                "category": "stage_timeout",
                "message": "native stage timed out",
                "tool": "thread_read",
                "stage": "thread/read"
            })
        );
    }

    #[test]
    fn fixed_errors_do_not_serialize_sensitive_inputs() {
        let prompt = "PROMPT_SENTINEL";
        let credential = "CREDENTIAL_SENTINEL";
        let environment = "ENVIRONMENT_SENTINEL";
        let backtrace = "BACKTRACE_SENTINEL";

        let error = ToolErrorData::fixed(
            ToolErrorCategory::TargetUnavailable,
            "thread_read",
            "connect",
        );
        let rendered = format!("{error:?} {}", serde_json::to_string(&error).unwrap());
        for sensitive in [prompt, credential, environment, backtrace] {
            assert!(!rendered.contains(sensitive));
        }
    }
}
