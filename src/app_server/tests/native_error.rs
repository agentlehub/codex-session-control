use super::*;

#[test]
fn only_exact_fixture_error_exemplars_receive_special_categories() {
    let fixture = protocol_fixture();
    let missing = &fixture.error_exemplars["threadNotFound"];
    let conflict = &fixture.error_exemplars["activeTurnMismatch"];
    assert_eq!(fixture.codex_version, "0.146.0");
    assert_eq!(fixture.schema_sha256.len(), 64);
    assert!(fixture.successful_exemplars.contains_key("initialize"));
    assert!(fixture.turns_newest_first);

    assert_eq!(
        classify_fixture_error("thread/read", missing, fixture),
        ToolErrorCategory::TargetUnavailable
    );
    assert_eq!(
        classify_fixture_error("turn/steer", conflict, fixture),
        ToolErrorCategory::NativeConflict
    );
    assert_eq!(
        classify_native_error(
            "thread/read",
            missing["code"].as_i64().unwrap(),
            "different message",
            missing.get("data"),
            fixture,
        ),
        ToolErrorCategory::TargetUnavailable
    );
    assert_eq!(
        classify_native_error(
            "thread/read",
            missing["code"].as_i64().unwrap(),
            missing["message"].as_str().unwrap(),
            Some(&Value::Null),
            fixture,
        ),
        ToolErrorCategory::NativeError
    );
    assert_eq!(
        classify_fixture_error("thread/list", missing, fixture),
        ToolErrorCategory::NativeError
    );
    assert_eq!(
        classify_fixture_error("thread/metadata/update", missing, fixture),
        ToolErrorCategory::NativeError
    );
    assert_eq!(
        native_error(
            "thread_read",
            "thread/read",
            missing,
            false,
            Some("thread-1"),
            None,
        )
        .category,
        ToolErrorCategory::TargetUnavailable
    );
}
