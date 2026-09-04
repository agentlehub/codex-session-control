use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ThreadStatus {
    NotLoaded,
    Idle,
    SystemError,
    Active { active_flags: Vec<ActiveFlag> },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ActiveFlag {
    WaitingOnApproval,
    WaitingOnUserInput,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Thread {
    pub id: String,
    pub name: Option<String>,
    pub preview: String,
    pub cwd: String,
    pub status: ThreadStatus,
    pub created_at: i64,
    pub updated_at: i64,
    pub forked_from_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    Completed,
    Interrupted,
    Failed,
    InProgress,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnItemsView {
    NotLoaded,
    Summary,
    Full,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Turn {
    pub id: String,
    pub status: TurnStatus,
    #[schemars(with = "Vec<std::collections::BTreeMap<String, Value>>")]
    pub items: Vec<Value>,
    pub items_view: TurnItemsView,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub duration_ms: Option<i64>,
    #[schemars(with = "Option<std::collections::BTreeMap<String, Value>>")]
    pub error: Option<Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadGoalStatus {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadGoal {
    pub thread_id: String,
    pub objective: String,
    pub status: ThreadGoalStatus,
    pub token_budget: Option<u64>,
    pub tokens_used: u64,
    pub time_used_seconds: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSnapshot {
    pub thread_id: String,
    pub name: Option<String>,
    pub status: ThreadStatus,
    pub active_turn_id: Option<String>,
    pub active_turn_status: Option<TurnStatus>,
    pub updated_at: i64,
}

#[cfg(test)]
mod tests {
    use schemars::schema_for;
    use serde_json::{Value, json};

    use super::*;

    const GOAL_STATUSES: [(&str, ThreadGoalStatus); 6] = [
        ("active", ThreadGoalStatus::Active),
        ("paused", ThreadGoalStatus::Paused),
        ("blocked", ThreadGoalStatus::Blocked),
        ("usageLimited", ThreadGoalStatus::UsageLimited),
        ("budgetLimited", ThreadGoalStatus::BudgetLimited),
        ("complete", ThreadGoalStatus::Complete),
    ];

    #[test]
    fn stable_enums_use_exact_public_names() {
        for (expected, status) in GOAL_STATUSES {
            assert_eq!(serde_json::to_value(status).unwrap(), json!(expected));
        }

        for (status, expected) in [
            (ThreadStatus::NotLoaded, json!({"type": "notLoaded"})),
            (ThreadStatus::Idle, json!({"type": "idle"})),
            (ThreadStatus::SystemError, json!({"type": "systemError"})),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), expected);
        }

        for (flag, expected) in [
            (ActiveFlag::WaitingOnApproval, "waitingOnApproval"),
            (ActiveFlag::WaitingOnUserInput, "waitingOnUserInput"),
        ] {
            assert_eq!(serde_json::to_value(flag).unwrap(), json!(expected));
        }

        for (status, expected) in [
            (TurnStatus::Completed, "completed"),
            (TurnStatus::Interrupted, "interrupted"),
            (TurnStatus::Failed, "failed"),
            (TurnStatus::InProgress, "inProgress"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), json!(expected));
        }

        for (view, expected) in [
            (TurnItemsView::NotLoaded, "notLoaded"),
            (TurnItemsView::Summary, "summary"),
            (TurnItemsView::Full, "full"),
        ] {
            assert_eq!(serde_json::to_value(view).unwrap(), json!(expected));
        }
    }

    #[test]
    fn active_status_and_schema_use_active_flags_in_camel_case() {
        let status = ThreadStatus::Active {
            active_flags: vec![ActiveFlag::WaitingOnApproval],
        };
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            json!({"type": "active", "activeFlags": ["waitingOnApproval"]})
        );

        let schema = serde_json::to_value(schema_for!(ThreadStatus)).unwrap();
        let encoded = serde_json::to_string(&schema).unwrap();
        assert!(encoded.contains("\"activeFlags\""));
        assert!(!encoded.contains("active_flags"));
    }

    #[test]
    fn missing_optional_native_fields_normalize_to_json_null() {
        let thread = Thread {
            id: "thread-1".to_owned(),
            name: None,
            preview: String::new(),
            cwd: "/tmp".to_owned(),
            status: ThreadStatus::Idle,
            created_at: 1,
            updated_at: 2,
            forked_from_id: None,
        };
        assert_eq!(
            serde_json::to_value(thread).unwrap(),
            json!({
                "id": "thread-1",
                "name": null,
                "preview": "",
                "cwd": "/tmp",
                "status": {"type": "idle"},
                "createdAt": 1,
                "updatedAt": 2,
                "forkedFromId": null
            })
        );

        let turn = Turn {
            id: "turn-1".to_owned(),
            status: TurnStatus::Completed,
            items: Vec::<Value>::new(),
            items_view: TurnItemsView::NotLoaded,
            started_at: None,
            completed_at: None,
            duration_ms: None,
            error: None,
        };
        assert_eq!(
            serde_json::to_value(turn).unwrap(),
            json!({
                "id": "turn-1",
                "status": "completed",
                "items": [],
                "itemsView": "notLoaded",
                "startedAt": null,
                "completedAt": null,
                "durationMs": null,
                "error": null
            })
        );

        let goal = ThreadGoal {
            thread_id: "thread-1".to_owned(),
            objective: "ship".to_owned(),
            status: ThreadGoalStatus::Active,
            token_budget: None,
            tokens_used: 3,
            time_used_seconds: 4,
            created_at: 5,
            updated_at: 6,
        };
        assert_eq!(
            serde_json::to_value(goal).unwrap(),
            json!({
                "threadId": "thread-1",
                "objective": "ship",
                "status": "active",
                "tokenBudget": null,
                "tokensUsed": 3,
                "timeUsedSeconds": 4,
                "createdAt": 5,
                "updatedAt": 6
            })
        );

        let snapshot = ThreadSnapshot {
            thread_id: "thread-1".to_owned(),
            name: None,
            status: ThreadStatus::Idle,
            active_turn_id: None,
            active_turn_status: None,
            updated_at: 7,
        };
        assert_eq!(
            serde_json::to_value(snapshot).unwrap(),
            json!({
                "threadId": "thread-1",
                "name": null,
                "status": {"type": "idle"},
                "activeTurnId": null,
                "activeTurnStatus": null,
                "updatedAt": 7
            })
        );
    }
}
