use super::*;

#[test]
fn validation_requires_well_formed_caller_only_at_caller_boundaries() {
    for malformed in [
        Value::Null,
        json!({}),
        json!({"threadId": null}),
        json!({"threadId": 4}),
        json!({"threadId": ""}),
        json!({"threadId": "caller id"}),
    ] {
        let error = validate_input(
            "thread_message_send",
            arguments(json!({"threadId": "target", "prompt": "hello"})),
            &malformed,
        )
        .unwrap_err();
        assert_category(error, ToolErrorCategory::InvalidRequest);
    }

    validate_input(
        "thread_create",
        arguments(json!({"prompt": "hello", "cwd": "/tmp"})),
        &Value::Null,
    )
    .unwrap();
    validate_input(
        "thread_read",
        arguments(json!({"threadId": "target"})),
        &Value::Null,
    )
    .unwrap();
}

#[test]
fn validation_resolves_omitted_metadata_targets_and_allows_explicit_self() {
    for (tool, args) in [
        ("thread_fork", json!({})),
        ("thread_title_set", json!({"title": "title"})),
    ] {
        assert_category(
            validate_input(tool, arguments(args), &Value::Null).unwrap_err(),
            ToolErrorCategory::InvalidRequest,
        );
    }

    let fork = validate_input(
        "thread_fork",
        arguments(json!({"threadId": "caller"})),
        &meta("caller"),
    )
    .unwrap();
    let ValidatedInput::ThreadFork(fork) = fork else {
        panic!("wrong validated input")
    };
    assert_eq!(fork.thread_id.as_deref(), Some("caller"));

    let title = validate_input(
        "thread_title_set",
        arguments(json!({"threadId": "caller", "title": " title "})),
        &meta("caller"),
    )
    .unwrap();
    let ValidatedInput::ThreadTitleSet(title) = title else {
        panic!("wrong validated input")
    };
    assert_eq!(title.thread_id.as_deref(), Some("caller"));
    assert_eq!(title.title, " title ");
}

#[test]
fn validation_defaults_deferred_goal_continuation_to_true() {
    let omitted = validate_input("thread_fork", arguments(json!({})), &meta("caller")).unwrap();
    let ValidatedInput::ThreadFork(omitted) = omitted else {
        panic!("wrong validated input")
    };
    assert_eq!(omitted.thread_id.as_deref(), Some("caller"));
    assert!(omitted.defer_goal_continuation);

    let explicit = validate_input(
        "thread_fork",
        arguments(json!({"deferGoalContinuation": false})),
        &meta("caller"),
    )
    .unwrap();
    let ValidatedInput::ThreadFork(explicit) = explicit else {
        panic!("wrong validated input")
    };
    assert!(!explicit.defer_goal_continuation);
}

#[test]
fn validation_rejects_self_for_wait_message_goals_and_interrupt() {
    let cases = [
        ("threads_wait", json!({"threadIds": ["caller"]})),
        (
            "thread_message_send",
            json!({"threadId": "caller", "prompt": "hello"}),
        ),
        ("thread_goal_get", json!({"threadId": "caller"})),
        (
            "thread_goal_set",
            json!({"threadId": "caller", "objective": "goal"}),
        ),
        ("thread_goal_pause", json!({"threadId": "caller"})),
        ("thread_goal_resume", json!({"threadId": "caller"})),
        ("thread_goal_clear", json!({"threadId": "caller"})),
        ("thread_interrupt", json!({"threadId": "caller"})),
    ];
    for (tool, args) in cases {
        assert_category(
            validate_input(tool, arguments(args), &meta("caller")).unwrap_err(),
            ToolErrorCategory::PolicyRejected,
        );
    }
}

#[test]
fn validation_accepts_one_through_eight_unique_wait_ids_and_bounded_timeouts() {
    for count in 1..=8 {
        let ids: Vec<String> = (0..count).map(|index| format!("target-{index}")).collect();
        let validated = validate_input(
            "threads_wait",
            arguments(json!({"threadIds": ids})),
            &meta("caller"),
        )
        .unwrap();
        let ValidatedInput::ThreadsWait { input, timeout } = validated else {
            panic!("wrong validated input")
        };
        assert_eq!(input.thread_ids.len(), count);
        assert_eq!(timeout, Duration::from_millis(DEFAULT_WAIT_TIMEOUT_MS));
    }

    for args in [
        json!({"threadIds": []}),
        json!({"threadIds": ["a", "b", "c", "d", "e", "f", "g", "h", "i"]}),
        json!({"threadIds": ["target", "target"]}),
        json!({"threadIds": ["target"], "timeoutMs": MAX_WAIT_TIMEOUT_MS + 1}),
    ] {
        assert_category(
            validate_input("threads_wait", arguments(args), &meta("caller")).unwrap_err(),
            ToolErrorCategory::InvalidRequest,
        );
    }

    for timeout_ms in [0, MAX_WAIT_TIMEOUT_MS] {
        let validated = validate_input(
            "threads_wait",
            arguments(json!({"threadIds": ["target"], "timeoutMs": timeout_ms})),
            &meta("caller"),
        )
        .unwrap();
        let ValidatedInput::ThreadsWait { timeout, .. } = validated else {
            panic!("wrong validated input")
        };
        assert_eq!(timeout, Duration::from_millis(timeout_ms));
    }
}

#[test]
fn validation_enforces_id_cursor_cwd_limit_and_open_effort_shapes() {
    for (tool, args, caller) in [
        ("thread_read", json!({"threadId": ""}), Value::Null),
        (
            "thread_read",
            json!({"threadId": "target", "cursor": "bad cursor"}),
            Value::Null,
        ),
        ("threads_list", json!({"cursor": "\u{2003}"}), Value::Null),
        (
            "thread_message_send",
            json!({"threadId": "bad id", "prompt": "hello"}),
            meta("caller"),
        ),
        (
            "thread_create",
            json!({"prompt": "hello", "cwd": "relative"}),
            Value::Null,
        ),
        (
            "thread_create",
            json!({"prompt": "hello", "cwd": "/tmp", "reasoningEffort": ""}),
            Value::Null,
        ),
    ] {
        assert_category(
            validate_input(tool, arguments(args), &caller).unwrap_err(),
            ToolErrorCategory::InvalidRequest,
        );
    }

    for (tool, args) in [
        ("threads_list", json!({"limit": u64::from(u32::MAX) + 1})),
        (
            "thread_read",
            json!({"threadId": "target", "limit": u64::from(u32::MAX) + 1}),
        ),
    ] {
        assert_category(
            validate_input(tool, arguments(args), &Value::Null).unwrap_err(),
            ToolErrorCategory::InvalidRequest,
        );
    }

    validate_input(
        "threads_list",
        arguments(json!({"limit": u32::MAX})),
        &Value::Null,
    )
    .unwrap();
    let create = validate_input(
        "thread_create",
        arguments(json!({
            "prompt": " \n prompt \t ",
            "cwd": "/tmp",
            "model": "",
            "reasoningEffort": " "
        })),
        &Value::Null,
    )
    .unwrap();
    let ValidatedInput::ThreadCreate(create) = create else {
        panic!("wrong validated input")
    };
    assert_eq!(create.prompt, " \n prompt \t ");
    assert_eq!(create.cwd, "/tmp");
    assert_eq!(create.model.as_deref(), Some(""));
    assert_eq!(create.reasoning_effort.as_deref(), Some(" "));

    let goal = validate_input(
        "thread_goal_set",
        arguments(json!({"threadId": "target", "objective": " \n goal \t "})),
        &meta("caller"),
    )
    .unwrap();
    let ValidatedInput::ThreadGoalSet(goal) = goal else {
        panic!("wrong validated input")
    };
    assert_eq!(goal.objective, " \n goal \t ");
}

#[test]
fn validation_rejects_unknown_public_fields_for_every_tool() {
    let cases = [
        ("thread_create", json!({"prompt": "p", "cwd": "/tmp"})),
        ("thread_fork", json!({"threadId": "target"})),
        ("threads_list", json!({})),
        ("thread_read", json!({"threadId": "target"})),
        ("threads_wait", json!({"threadIds": ["target"]})),
        (
            "thread_message_send",
            json!({"threadId": "target", "prompt": "p"}),
        ),
        (
            "thread_title_set",
            json!({"threadId": "target", "title": "t"}),
        ),
        ("thread_goal_get", json!({"threadId": "target"})),
        (
            "thread_goal_set",
            json!({"threadId": "target", "objective": "o"}),
        ),
        ("thread_goal_pause", json!({"threadId": "target"})),
        ("thread_goal_resume", json!({"threadId": "target"})),
        ("thread_goal_clear", json!({"threadId": "target"})),
        ("thread_interrupt", json!({"threadId": "target"})),
    ];

    for (tool, mut args) in cases {
        args.as_object_mut()
            .unwrap()
            .insert("unknown".to_owned(), json!(true));
        assert_category(
            validate_input(tool, arguments(args), &meta("caller")).unwrap_err(),
            ToolErrorCategory::InvalidRequest,
        );
    }
}

#[test]
fn validation_rejects_explicit_null_for_every_optional_public_field() {
    for (tool, args) in [
        (
            "thread_create",
            json!({"prompt": "p", "cwd": "/tmp", "model": null}),
        ),
        (
            "thread_create",
            json!({"prompt": "p", "cwd": "/tmp", "reasoningEffort": null}),
        ),
        ("thread_fork", json!({"threadId": null})),
        (
            "thread_fork",
            json!({"threadId": "target", "deferGoalContinuation": null}),
        ),
        ("threads_list", json!({"cursor": null})),
        ("threads_list", json!({"limit": null})),
        ("threads_list", json!({"archived": null})),
        ("threads_list", json!({"cwd": null})),
        ("thread_read", json!({"threadId": "target", "cursor": null})),
        ("thread_read", json!({"threadId": "target", "limit": null})),
        (
            "thread_read",
            json!({"threadId": "target", "itemsView": null}),
        ),
        (
            "threads_wait",
            json!({"threadIds": ["target"], "timeoutMs": null}),
        ),
        (
            "thread_message_send",
            json!({"threadId": "target", "prompt": "p", "model": null}),
        ),
        (
            "thread_message_send",
            json!({"threadId": "target", "prompt": "p", "reasoningEffort": null}),
        ),
        ("thread_title_set", json!({"threadId": null, "title": "t"})),
    ] {
        assert_category(
            validate_input(tool, arguments(args), &meta("caller")).unwrap_err(),
            ToolErrorCategory::InvalidRequest,
        );
    }
}

#[test]
fn validation_warning_prefixes_preserve_structured_success_and_error_bytes() {
    let untested_version = crate::test_support::different_stable_version(TESTED_CODEX_VERSION);
    let warnings = [untested_version.as_str(), "unknown"].map(|version| {
        format!(
            "WARNING: Target Codex {version} is untested. Codex session control was validated against Codex {TESTED_CODEX_VERSION}. Report this warning to the operator. The accompanying structured data remains authoritative."
        )
    });

    for (tool, _, _) in TOOL_EFFECTS {
        let structured = json!({"tool": tool, "value": [3, 2, 1]});
        let structured_bytes = serde_json::to_vec(&structured).unwrap();
        let error = ToolErrorData::fixed(ToolErrorCategory::TargetUnavailable, tool, "connect");
        let error_value = serde_json::to_value(&error).unwrap();
        let error_bytes = serde_json::to_vec(&error_value).unwrap();

        for warning in &warnings {
            let success = success_result(structured.clone(), Some(warning));
            assert_eq!(
                serde_json::to_vec(success.structured_content.as_ref().unwrap()).unwrap(),
                structured_bytes
            );
            let rendered = serde_json::to_value(&success).unwrap();
            assert!(
                rendered["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .starts_with(warning)
            );

            let failure = error_response(error.clone(), Some(warning));
            assert_eq!(failure.code, ErrorCode(-32000));
            assert!(failure.message.starts_with(warning));
            assert_eq!(
                serde_json::to_vec(failure.data.as_ref().unwrap()).unwrap(),
                error_bytes
            );
        }

        let success = success_result(structured.clone(), None);
        assert_eq!(
            serde_json::to_vec(success.structured_content.as_ref().unwrap()).unwrap(),
            structured_bytes
        );
        let rendered = serde_json::to_value(&success).unwrap();
        assert!(
            !rendered["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("WARNING:")
        );

        let failure = error_response(error.clone(), None);
        assert!(!failure.message.starts_with("WARNING:"));
        assert_eq!(
            serde_json::to_vec(failure.data.as_ref().unwrap()).unwrap(),
            error_bytes
        );
    }
}

#[test]
fn validation_uses_exact_mcp_error_codes_and_structured_data() {
    let invalid = invalid_request("thread_read", "input");
    let invalid_json = serde_json::to_value(&invalid).unwrap();
    let response = error_response(invalid, None);
    assert_eq!(response.code, ErrorCode::INVALID_PARAMS);
    assert_eq!(response.data, Some(invalid_json));

    let unavailable = ToolErrorData::fixed(
        ToolErrorCategory::TargetUnavailable,
        "thread_read",
        "configuration",
    );
    let unavailable_json = serde_json::to_value(&unavailable).unwrap();
    let response = error_response(unavailable, None);
    assert_eq!(response.code, ErrorCode(-32000));
    assert_eq!(response.data, Some(unavailable_json));
}

#[test]
fn validation_catalog_has_one_schema_per_exact_tool() {
    let tools = catalog();
    let by_name: BTreeMap<_, _> = tools
        .iter()
        .map(|tool| (tool.name.as_ref(), tool))
        .collect();
    assert_eq!(by_name.len(), TOOL_EFFECTS.len());
    for (name, _, _) in TOOL_EFFECTS {
        let tool = by_name.get(name).unwrap();
        assert_eq!(
            tool.input_schema.get("additionalProperties"),
            Some(&json!(false))
        );
        assert!(tool.output_schema.is_some());
        assert!(!tool.input_schema.contains_key("callerThreadId"));
    }
}
