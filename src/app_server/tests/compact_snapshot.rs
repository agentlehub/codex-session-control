use serde_json::{Value, json};

use super::super::protocol::compact_snapshot_from_native;
use crate::{
    error::ToolErrorCategory,
    model::{ThreadStatus, TurnStatus},
};

fn metadata(id: Option<&str>, status: Value) -> Value {
    let mut thread = json!({
        "name": null,
        "status": status,
        "updatedAt": 42,
    });
    if let Some(id) = id {
        thread["id"] = json!(id);
    }
    json!({"thread": thread})
}

#[test]
fn compact_snapshot_rejects_every_malformed_minimum_shape() {
    let cases = [
        (
            "missing returned thread id",
            metadata(None, json!({"type": "idle"})),
            json!({"data": []}),
            "thread/read",
        ),
        (
            "mismatched returned thread id",
            metadata(Some("wrong"), json!({"type": "idle"})),
            json!({"data": []}),
            "thread/read",
        ),
        (
            "missing latest data",
            metadata(Some("target"), json!({"type": "idle"})),
            json!({}),
            "thread/turns/list",
        ),
        (
            "non-array latest data",
            metadata(Some("target"), json!({"type": "idle"})),
            json!({"data": {}}),
            "thread/turns/list",
        ),
        (
            "missing latest turn status",
            metadata(Some("target"), json!({"type": "active", "activeFlags": []})),
            json!({"data": [{"id": "turn-1"}]}),
            "thread/turns/list",
        ),
        (
            "unknown latest turn status",
            metadata(Some("target"), json!({"type": "active", "activeFlags": []})),
            json!({"data": [{"id": "turn-1", "status": "unknown"}]}),
            "thread/turns/list",
        ),
        (
            "in-progress latest turn without id",
            metadata(Some("target"), json!({"type": "active", "activeFlags": []})),
            json!({"data": [{"status": "inProgress"}]}),
            "thread/turns/list",
        ),
    ];

    let failures = cases
        .into_iter()
        .filter_map(|(name, metadata, latest, expected_stage)| {
            match compact_snapshot_from_native("target", &metadata, &latest) {
                Err(error)
                    if error.category == ToolErrorCategory::NativeError
                        && error.tool == expected_stage
                        && error.stage == expected_stage =>
                {
                    None
                }
                result => Some(format!("{name}: {result:?}")),
            }
        })
        .collect::<Vec<_>>();

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn compact_snapshot_preserves_demonstrated_valid_native_variants() {
    let cases = [
        (
            "empty history",
            json!({"type": "idle"}),
            json!({"data": []}),
            ThreadStatus::Idle,
            None,
            None,
        ),
        (
            "active latest turn",
            json!({"type": "active", "activeFlags": []}),
            json!({"data": [{"id": "active", "status": "inProgress"}]}),
            ThreadStatus::Active {
                active_flags: Vec::new(),
            },
            Some("active"),
            Some(TurnStatus::InProgress),
        ),
        (
            "completed latest turn",
            json!({"type": "idle"}),
            json!({"data": [{"id": "completed", "status": "completed"}]}),
            ThreadStatus::Idle,
            None,
            None,
        ),
        (
            "interrupted latest turn",
            json!({"type": "idle"}),
            json!({"data": [{"id": "interrupted", "status": "interrupted"}]}),
            ThreadStatus::Idle,
            None,
            None,
        ),
        (
            "failed latest turn",
            json!({"type": "systemError"}),
            json!({"data": [{"id": "failed", "status": "failed"}]}),
            ThreadStatus::SystemError,
            None,
            None,
        ),
    ];

    for (name, status, latest, expected_status, expected_turn_id, expected_turn_status) in cases {
        let snapshot =
            compact_snapshot_from_native("target", &metadata(Some("target"), status), &latest)
                .unwrap_or_else(|error| panic!("{name}: {error:?}"));

        assert_eq!(snapshot.thread_id, "target", "{name}");
        assert_eq!(snapshot.status, expected_status, "{name}");
        assert_eq!(
            snapshot.active_turn_id.as_deref(),
            expected_turn_id,
            "{name}"
        );
        assert_eq!(snapshot.active_turn_status, expected_turn_status, "{name}");
        assert_eq!(snapshot.updated_at, 42, "{name}");
    }
}
