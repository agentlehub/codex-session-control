use std::{error::Error, fs, os::unix::fs::PermissionsExt, panic::AssertUnwindSafe};

use futures_util::FutureExt;
use serde_json::{Value, json};

use crate::live_harness::{
    ALL_SOURCE_KINDS, EXPECTED_CODEX_VERSION, LiveHarness,
    assert_shutdown_precedes_descriptor_removal, request_has_exact_session_control_tools,
    request_has_session_control_tools, request_session_control_tool_names,
};
use crate::normal_home::{
    DisposableNormalHome, combine_cleanup_results, require_normal_home_ci_opt_ins,
};
use crate::normal_home_paths::DISPOSABLE_CLI_CANARY_OPT_IN;
use crate::protocol_support::ResponsesEndpoint;

pub(super) async fn live_schema_digest_matches_committed_fixture() -> Result<(), Box<dyn Error>> {
    let live = LiveHarness::start().await?;
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/app-server-contract.json"))?;

    assert_eq!(live.codex_version(), EXPECTED_CODEX_VERSION);
    assert_eq!(
        live.schema_digest().await?,
        fixture["schemaSha256"].as_str().unwrap()
    );
    live.assert_fixture_clean()?;
    Ok(())
}

pub(super) async fn live_read_list_fork_title_pin_goal_interrupt_mappings()
-> Result<(), Box<dyn Error>> {
    let live = LiveHarness::start().await?;
    let mut native = live.connect().await?;
    let thread_id = live.start_thread(&mut native).await?;
    let persisted_turn = live
        .start_turn(&mut native, &thread_id, "PERSIST_FOR_THREAD_LIST")
        .await?;
    live.endpoint().wait_for_requests(1).await?;
    live.wait_for_turn_status(&mut native, &thread_id, &persisted_turn, "completed")
        .await?;

    let listed = live.wait_for_thread_list(&mut native, &thread_id).await?;
    assert!(
        listed["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|thread| thread["id"].as_str() == Some(&thread_id))
    );
    assert_eq!(
        native
            .request(
                "thread/read",
                json!({"threadId": thread_id, "includeTurns": false}),
            )
            .await?["thread"]["id"],
        thread_id
    );

    let fork = native
        .request(
            "thread/fork",
            json!({"threadId": thread_id, "deferGoalContinuation": false}),
        )
        .await?;
    assert_eq!(fork["thread"]["forkedFromId"], thread_id);
    native
        .request(
            "thread/name/set",
            json!({"threadId": thread_id, "name": "Live mapping title"}),
        )
        .await?;
    assert_eq!(
        native
            .request(
                "thread/read",
                json!({"threadId": thread_id, "includeTurns": false}),
            )
            .await?["thread"]["name"],
        "Live mapping title"
    );

    let pinned: Value = native
        .request(
            "thread/section/move",
            json!({
                "threadId": thread_id,
                "sectionId": "01984de2-8f74-7c91-a3b2-5c5e937cf318",
            }),
        )
        .await?;
    assert!(pinned.is_object());
    assert_eq!(
        native
            .request(
                "thread/read",
                json!({"threadId": thread_id, "includeTurns": false}),
            )
            .await?["thread"]["section"]["id"],
        "01984de2-8f74-7c91-a3b2-5c5e937cf318"
    );

    let unpinned: Value = native
        .request(
            "thread/section/move",
            json!({"threadId": thread_id, "sectionId": null}),
        )
        .await?;
    assert!(unpinned.is_object());
    assert!(
        native
            .request(
                "thread/read",
                json!({"threadId": thread_id, "includeTurns": false}),
            )
            .await?["thread"]["section"]
            .is_null()
    );

    let goal = native
        .request(
            "thread/goal/set",
            json!({"threadId": thread_id, "objective": "Live goal", "status": "paused"}),
        )
        .await?;
    assert_eq!(goal["goal"]["status"], "paused");
    assert_eq!(
        native
            .request("thread/goal/get", json!({"threadId": thread_id}))
            .await?["goal"]["objective"],
        "Live goal"
    );

    let interrupt_thread_id = live.start_thread(&mut native).await?;
    let turn_id = live
        .start_turn(&mut native, &interrupt_thread_id, "HOLD_SESSION_CONTROL")
        .await?;
    live.endpoint().wait_for_requests(2).await?;
    assert_eq!(
        native
            .request(
                "thread/turns/list",
                json!({"threadId": interrupt_thread_id, "limit": 1, "itemsView": "notLoaded"}),
            )
            .await?["data"][0]["status"],
        "inProgress"
    );
    native
        .request(
            "turn/interrupt",
            json!({"threadId": interrupt_thread_id, "turnId": turn_id}),
        )
        .await?;
    live.wait_for_turn_status(&mut native, &interrupt_thread_id, &turn_id, "interrupted")
        .await?;
    live.assert_fixture_clean()?;
    Ok(())
}

pub(super) async fn live_remote_cli_attaches_and_reconnects() -> Result<(), Box<dyn Error>> {
    let live = LiveHarness::start().await?;
    let authority = live.identity()?;

    let mut first = live.spawn_remote("REMOTE_ATTACH_ONE").await?;
    live.endpoint().wait_for_requests(1).await?;
    first.stop().await?;
    assert_eq!(live.identity()?, authority);

    let mut second = live.spawn_remote("REMOTE_ATTACH_TWO").await?;
    live.endpoint().wait_for_requests(2).await?;
    second.stop().await?;
    assert_eq!(live.identity()?, authority);
    live.assert_fixture_clean()?;
    Ok(())
}

pub(super) async fn live_restart_preserves_shared_home_sessions() -> Result<(), Box<dyn Error>> {
    let mut live = LiveHarness::start().await?;
    let mut native = live.connect().await?;
    let thread_id = live.start_thread(&mut native).await?;
    let turn_id = live
        .start_turn(&mut native, &thread_id, "PERSIST_BEFORE_RESTART")
        .await?;
    live.endpoint().wait_for_requests(1).await?;
    live.wait_for_turn_status(&mut native, &thread_id, &turn_id, "completed")
        .await?;
    assert!(live.codex_home_contains_session()?);
    let before = live.identity()?;
    drop(native);

    live.restart().await?;

    let after = live.identity()?;
    assert_ne!(before, after);
    assert_eq!(
        live.connect()
            .await?
            .request(
                "thread/read",
                json!({"threadId": thread_id, "includeTurns": false}),
            )
            .await?["thread"]["id"],
        thread_id
    );
    assert!(live.codex_home_contains_session()?);
    live.assert_fixture_clean()?;
    Ok(())
}

pub(super) async fn live_socket_removed_when_app_server_exits() -> Result<(), Box<dyn Error>> {
    let mut live = LiveHarness::start().await?;
    assert!(live.socket_path().exists());
    let socket_mode = fs::symlink_metadata(live.socket_path())?
        .permissions()
        .mode()
        & 0o777;
    assert!(matches!(socket_mode, 0o600 | 0o700));

    live.stop().await?;

    assert!(!live.socket_path().exists());
    live.assert_fixture_clean()?;
    Ok(())
}

pub(super) async fn live_projection_converges_on_new_task_without_restart()
-> Result<(), Box<dyn Error>> {
    let live = LiveHarness::start().await?;
    let mut native = live.connect().await?;
    let old_thread = live.start_thread(&mut native).await?;
    live.start_turn(&mut native, &old_thread, "BEFORE_PROJECTION")
        .await?;
    let before = live.endpoint().wait_for_request(1).await?;
    let authority = live.identity()?;
    assert!(!request_has_session_control_tools(&before));

    live.install_projection().await?;
    assert_eq!(live.identity()?, authority);
    let new_thread = live.start_thread(&mut native).await?;
    live.start_turn(&mut native, &new_thread, "AFTER_PROJECTION")
        .await?;
    let after = live.endpoint().wait_for_request(2).await?;
    assert!(
        request_has_exact_session_control_tools(&after),
        "observed tools: {}\napp-server stderr: {}",
        serde_json::to_string_pretty(&after["tools"]).unwrap(),
        fs::read_to_string(&live.stderr_log).unwrap_or_default()
    );

    live.start_turn(&mut native, &old_thread, "OLD_AFTER_PROJECTION")
        .await?;
    let old_after = live.endpoint().wait_for_request(3).await?;
    assert!(!request_has_session_control_tools(&old_after));
    assert_eq!(live.identity()?, authority);
    live.assert_projected_fixture_clean()?;
    Ok(())
}

pub(super) async fn live_remote_cli_executes_projected_goal_tool_round_trip()
-> Result<(), Box<dyn Error>> {
    const OBJECTIVE: &str = "Live projected goal mutation";

    if std::env::var_os(DISPOSABLE_CLI_CANARY_OPT_IN).as_deref() != Some(std::ffi::OsStr::new("1"))
    {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI must not skip the disposable CLI composition canary"
        );
        eprintln!("SKIP: disposable CLI composition prerequisites unavailable");
        return Ok(());
    }

    let mut live = LiveHarness::start_disposable_ci().await?;
    let mut native = live.connect().await?;
    let target_thread = live.start_thread(&mut native).await?;
    live.endpoint()
        .prepare_goal_canary(&target_thread, OBJECTIVE);
    live.install_projection().await?;

    let mut supervisor = live
        .spawn_remote(&format!(
            "SESSION_CONTROL_GOAL_CANARY target={target_thread} objective={OBJECTIVE}"
        ))
        .await?;
    let initial_request = live.endpoint().wait_for_request(1).await?;
    assert!(
        request_session_control_tool_names(&initial_request).contains("thread_goal_set"),
        "projected thread_goal_set was absent: {}",
        serde_json::to_string_pretty(&initial_request["tools"])?
    );

    let tool_result_request = live.endpoint().wait_for_goal_output_after(1).await?;
    let tool_outputs = tool_result_request["input"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["type"].as_str() == Some("function_call_output"))
        .collect::<Vec<_>>();
    assert_eq!(tool_outputs.len(), 1);
    assert_eq!(
        tool_outputs[0]["call_id"].as_str(),
        Some("call_session_control_goal")
    );
    let tool_output = tool_outputs[0]["output"]
        .as_str()
        .ok_or("tool result output was not a string")?;
    assert!(
        tool_output.contains(&target_thread),
        "tool output omitted target id: {tool_output}"
    );
    assert!(
        tool_output.contains(OBJECTIVE),
        "tool output omitted objective: {tool_output}"
    );

    assert_eq!(
        native
            .request("thread/goal/get", json!({"threadId": target_thread}),)
            .await?["goal"]["objective"],
        OBJECTIVE
    );

    let listed = native
        .request(
            "thread/list",
            json!({"limit": 10, "sourceKinds": ALL_SOURCE_KINDS}),
        )
        .await?;
    let supervisor_threads = listed["data"]
        .as_array()
        .ok_or("thread/list omitted data")?
        .iter()
        .filter_map(|thread| thread["id"].as_str())
        .filter(|thread_id| *thread_id != target_thread)
        .collect::<Vec<_>>();
    assert_eq!(supervisor_threads.len(), 1);
    let supervisor_thread = supervisor_threads[0];
    assert_ne!(supervisor_thread, target_thread);
    let turns = native
        .request(
            "thread/turns/list",
            json!({
                "threadId": supervisor_thread,
                "limit": 1,
                "itemsView": "notLoaded",
                "sortDirection": "desc"
            }),
        )
        .await?;
    let supervisor_turn = turns["data"][0]["id"]
        .as_str()
        .ok_or("supervisor turn id was unavailable")?;
    live.wait_for_turn_status(&mut native, supervisor_thread, supervisor_turn, "completed")
        .await?;

    supervisor.stop().await?;
    live.assert_projected_fixture_clean()?;
    drop(native);
    live.stop_and_clean_disposable().await?;
    Ok(())
}

pub(super) fn disposable_normal_home_contract() {
    let fixture = DisposableNormalHome::contract();
    assert!(fixture.live.is_none());
}

pub(super) async fn goal_output_barrier_ignores_interleaved_responses_requests()
-> Result<(), Box<dyn Error>> {
    let endpoint = ResponsesEndpoint::start().await?;
    let matching_output = json!({
        "type": "function_call_output",
        "call_id": "call_session_control_goal",
        "output": "exact"
    });
    let exact_request = json!({
        "marker": "exact",
        "input": [matching_output.clone()]
    });
    endpoint.requests.lock().unwrap().extend([
        json!({"marker": "observed", "input": []}),
        json!({"marker": "unrelated", "input": [{"type": "message"}]}),
        json!({
            "marker": "overfull",
            "input": [
                matching_output,
                {
                    "type": "function_call_output",
                    "call_id": "call_unrelated",
                    "output": "unrelated"
                }
            ]
        }),
        exact_request.clone(),
    ]);

    let found = endpoint.wait_for_goal_output_after(1).await?;

    assert_eq!(found, exact_request);
    endpoint.assert_clean()
}

pub(super) fn shutdown_receipts_require_the_operation_specific_stage() {
    use std::os::unix::process::ExitStatusExt;

    let receipt = |stderr: &str| std::process::Output {
        status: std::process::ExitStatus::from_raw(0),
        stdout: Vec::new(),
        stderr: stderr.as_bytes().to_vec(),
    };
    let disable = receipt(
        "completed: service-disable\ncompleted: service-stop-verify\ncompleted: descriptor-remove\n",
    );
    let uninstall = receipt("completed: service-stop\ncompleted: descriptor-remove\n");

    assert!(assert_shutdown_precedes_descriptor_removal("disable", &disable).is_ok());
    assert!(assert_shutdown_precedes_descriptor_removal("uninstall", &uninstall).is_ok());
    assert!(assert_shutdown_precedes_descriptor_removal("disable", &uninstall).is_err());
    assert!(assert_shutdown_precedes_descriptor_removal("uninstall", &disable).is_err());
}

pub(super) fn normal_home_ci_requires_each_exact_opt_in_before_mutation() {
    let enabled = Some(std::ffi::OsStr::new("1"));

    assert!(require_normal_home_ci_opt_ins(enabled, enabled, enabled).is_ok());
    assert!(require_normal_home_ci_opt_ins(None, enabled, enabled).is_err());
    assert!(require_normal_home_ci_opt_ins(enabled, None, enabled).is_err());
    assert!(
        require_normal_home_ci_opt_ins(enabled, enabled, None)
            .unwrap_err()
            .contains("systemd")
    );
    assert!(
        require_normal_home_ci_opt_ins(enabled, enabled, Some(std::ffi::OsStr::new("true")))
            .unwrap_err()
            .contains("systemd")
    );
}

pub(super) fn cleanup_combination_keeps_absence_verification_authoritative() {
    let (clean, absence_established) = combine_cleanup_results(Ok(()), Ok(()));
    assert!(clean.is_ok());
    assert!(absence_established);

    let (uninstall_failed, absence_established) =
        combine_cleanup_results(Err("uninstall receipt missing".into()), Ok(()));
    assert!(absence_established);
    assert!(
        uninstall_failed
            .unwrap_err()
            .contains("uninstall receipt missing")
    );

    let (absence_failed, absence_established) =
        combine_cleanup_results(Ok(()), Err("socket remains".into()));
    assert!(!absence_established);
    assert!(absence_failed.unwrap_err().contains("socket remains"));

    let (both_failed, absence_established) = combine_cleanup_results(
        Err("uninstall receipt missing".into()),
        Err("socket remains".into()),
    );

    assert!(!absence_established);
    let message = both_failed.unwrap_err();
    assert!(message.contains("uninstall receipt missing"));
    assert!(message.contains("socket remains"));
}

pub(super) async fn live_normal_home_shared_authority() -> Result<(), Box<dyn Error>> {
    let mut fixture = DisposableNormalHome::prepare_ci().await?;
    let proof = AssertUnwindSafe(async {
        fixture.setup_ci().await?;
        fixture.prove_shared_authority().await
    })
    .catch_unwind()
    .await;
    fixture.finish_proof(proof).await
}

pub(super) async fn live_normal_home_restart_boundaries() -> Result<(), Box<dyn Error>> {
    let mut fixture = DisposableNormalHome::prepare_ci().await?;
    let proof = AssertUnwindSafe(async {
        fixture.setup_ci().await?;
        fixture.prove_restart_boundaries().await
    })
    .catch_unwind()
    .await;
    fixture.finish_proof(proof).await
}

pub(super) async fn live_normal_home_projection_preservation() -> Result<(), Box<dyn Error>> {
    let mut fixture = DisposableNormalHome::prepare_ci().await?;
    let proof = AssertUnwindSafe(async {
        fixture.setup_ci().await?;
        fixture.prove_projection_preservation().await
    })
    .catch_unwind()
    .await;
    fixture.finish_proof(proof).await
}

pub(super) async fn live_normal_home_uninstall_preservation() -> Result<(), Box<dyn Error>> {
    let mut fixture = DisposableNormalHome::prepare_ci().await?;
    let proof = AssertUnwindSafe(async {
        fixture.setup_ci().await?;
        fixture.prove_uninstall_preservation().await
    })
    .catch_unwind()
    .await;
    fixture.finish_proof(proof).await
}
