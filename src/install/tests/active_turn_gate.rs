use std::{
    collections::VecDeque,
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::UnixListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

use super::support::{FakeAuthority, Fixture};
use super::*;

const ALL_SOURCE_KINDS: [&str; 10] = [
    "cli",
    "vscode",
    "exec",
    "appServer",
    "subAgent",
    "subAgentReview",
    "subAgentCompact",
    "subAgentThreadSpawn",
    "subAgentOther",
    "unknown",
];

struct ScriptedAuthority {
    task: tokio::task::JoinHandle<()>,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl ScriptedAuthority {
    async fn start(paths: &ResolvedUserPaths, responses: Vec<Value>) -> Self {
        Self::start_with_initialize_failures(paths, responses, Vec::new()).await
    }

    async fn start_with_initialize_failures(
        paths: &ResolvedUserPaths,
        responses: Vec<Value>,
        initialize_failures: Vec<bool>,
    ) -> Self {
        create_product_dir(&paths.runtime_dir, paths.euid).unwrap();
        if paths.socket.exists() {
            fs::remove_file(&paths.socket).unwrap();
        }
        let listener = UnixListener::bind(&paths.socket).unwrap();
        fs::set_permissions(&paths.socket, fs::Permissions::from_mode(0o600)).unwrap();
        let codex_home = paths.codex_home.clone();
        let responses = Arc::new(Mutex::new(VecDeque::from(responses)));
        let initialize_failures = Arc::new(Mutex::new(VecDeque::from(initialize_failures)));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let served_responses = Arc::clone(&responses);
        let served_initialize_failures = Arc::clone(&initialize_failures);
        let recorded_requests = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let codex_home = codex_home.clone();
                let served_responses = Arc::clone(&served_responses);
                let served_initialize_failures = Arc::clone(&served_initialize_failures);
                let recorded_requests = Arc::clone(&recorded_requests);
                tokio::spawn(async move {
                    let mut websocket = accept_async(stream).await.unwrap();
                    let Message::Text(initialize) = websocket.next().await.unwrap().unwrap() else {
                        panic!("expected initialize")
                    };
                    let initialize: Value = serde_json::from_str(initialize.as_str()).unwrap();
                    if served_initialize_failures
                        .lock()
                        .unwrap()
                        .pop_front()
                        .unwrap_or(false)
                    {
                        websocket
                            .send(Message::text(
                                json!({
                                    "id": initialize["id"],
                                    "error": {
                                        "code": -32603,
                                        "message": "initialize failed"
                                    }
                                })
                                .to_string(),
                            ))
                            .await
                            .unwrap();
                        return;
                    }
                    websocket
                        .send(Message::text(
                            json!({
                                "id": initialize["id"],
                                "result": {
                                    "codexHome": codex_home,
                                    "userAgent": TESTED_CODEX_CLI_VERSION
                                }
                            })
                            .to_string(),
                        ))
                        .await
                        .unwrap();
                    let initialized = websocket.next().await.unwrap().unwrap();
                    assert_eq!(
                        initialized.into_text().unwrap(),
                        json!({"method": "initialized"}).to_string()
                    );
                    while let Some(Ok(Message::Text(request))) = websocket.next().await {
                        let request: Value = serde_json::from_str(request.as_str()).unwrap();
                        assert_eq!(request["method"], "thread/list");
                        recorded_requests.lock().unwrap().push(request.clone());
                        let response = served_responses.lock().unwrap().pop_front().unwrap();
                        let envelope = match response.get("error") {
                            Some(error) => {
                                json!({"id": request["id"], "error": error.clone()})
                            }
                            None => json!({"id": request["id"], "result": response}),
                        };
                        websocket
                            .send(Message::text(envelope.to_string()))
                            .await
                            .unwrap();
                    }
                });
            }
        });
        Self { task, requests }
    }

    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for ScriptedAuthority {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn thread(id: &str, title: &str, status: Value) -> Value {
    json!({
        "id": id,
        "name": title,
        "preview": "",
        "cwd": "/tmp",
        "status": status,
        "createdAt": 1,
        "updatedAt": 2,
        "forkedFromId": null
    })
}

fn page(data: Vec<Value>, next_cursor: Option<&str>) -> Value {
    json!({"data": data, "nextCursor": next_cursor})
}

fn active(id: &str, title: &str) -> Value {
    thread(id, title, json!({"type": "active", "activeFlags": []}))
}

fn idle(id: &str, title: &str) -> Value {
    thread(id, title, json!({"type": "idle"}))
}

fn assert_exact_list_request(request: &Value, cursor: Option<&str>) {
    assert_eq!(request["params"]["archived"], false);
    assert_eq!(
        request["params"]["sourceKinds"],
        serde_json::to_value(ALL_SOURCE_KINDS).unwrap()
    );
    match cursor {
        Some(cursor) => assert_eq!(request["params"]["cursor"], cursor),
        None => assert!(request["params"].get("cursor").is_none()),
    }
    assert_eq!(
        request["params"].as_object().unwrap().len(),
        2 + usize::from(cursor.is_some())
    );
}

fn expected_prompt(tasks: &[(&str, &str)]) -> String {
    let mut prompt = format!(
        "Codex session control must restart its app-server to install this update.\n\
\n\
This will interrupt {} active tasks:\n",
        tasks.len()
    );
    for (id, title) in tasks {
        prompt.push_str(&format!("- {title} ({id})\n"));
    }
    prompt.push_str(
        "\n\
Their running turns will stop and be recorded as interrupted.\n\
\n\
Goals will not be paused or cleared. Restart alone will not continue them, but\n\
an active goal will start a new turn when a Codex client resumes its task.\n\
This can happen immediately if the task is already open.\n\
\n\
Pause any goal you do not want to continue before updating.\n\
\n\
Continue and interrupt active work? [y/N]",
    );
    prompt
}

#[tokio::test]
async fn pagination_requests_all_sources_and_keeps_only_normalized_active_threads() {
    let fixture = Fixture::new();
    let authority = ScriptedAuthority::start(
        &fixture.paths,
        vec![
            page(
                vec![active("thread-a", "Alpha"), idle("thread-idle", "Idle")],
                Some("page-2"),
            ),
            page(vec![active("thread-b", "Beta")], None),
        ],
    )
    .await;

    let active = list_active_threads(&fixture.paths, TESTED_CODEX_VERSION)
        .await
        .unwrap();

    assert_eq!(
        active
            .iter()
            .map(|thread| thread.id.as_str())
            .collect::<Vec<_>>(),
        ["thread-a", "thread-b"]
    );
    let requests = authority.requests();
    assert_eq!(requests.len(), 2);
    assert_exact_list_request(&requests[0], None);
    assert_exact_list_request(&requests[1], Some("page-2"));
}

#[tokio::test]
async fn page_and_decode_failures_abort_the_gate() {
    for response in [
        json!({"error": {"code": -32603, "message": "page failed"}}),
        json!({"data": "not-an-array", "nextCursor": null}),
    ] {
        let fixture = Fixture::new();
        let authority = ScriptedAuthority::start(&fixture.paths, vec![response]).await;

        let error = baseline_active_turn_gate(
            &fixture.paths,
            TESTED_CODEX_VERSION,
            TerminalState::noninteractive(),
        )
        .await
        .unwrap_err();

        assert_eq!(error.to_string(), "active task inspection failed");
        assert_eq!(authority.requests().len(), 1);
    }
}

#[tokio::test]
async fn initialize_failure_aborts_the_gate_before_listing() {
    let fixture = Fixture::new();
    let authority =
        ScriptedAuthority::start_with_initialize_failures(&fixture.paths, vec![], vec![true]).await;

    let error = baseline_active_turn_gate(
        &fixture.paths,
        TESTED_CODEX_VERSION,
        TerminalState::noninteractive(),
    )
    .await
    .unwrap_err();

    assert_eq!(error.to_string(), "active task inspection failed");
    assert!(authority.requests().is_empty());
}

#[tokio::test]
async fn zero_active_still_runs_one_final_complete_inspection() {
    let fixture = Fixture::new();
    let authority = ScriptedAuthority::start(
        &fixture.paths,
        vec![
            page(vec![idle("thread-idle", "Idle")], None),
            page(vec![], None),
        ],
    )
    .await;

    baseline_active_turn_gate(
        &fixture.paths,
        TESTED_CODEX_VERSION,
        TerminalState::noninteractive(),
    )
    .await
    .unwrap();

    assert_eq!(authority.requests().len(), 2);
}

#[tokio::test]
async fn noninteractive_and_declined_active_work_leave_the_gate_closed() {
    let fixture = Fixture::new();
    let noninteractive_authority = ScriptedAuthority::start(
        &fixture.paths,
        vec![page(vec![active("thread-a", "Alpha")], None)],
    )
    .await;
    let error = baseline_active_turn_gate(
        &fixture.paths,
        TESTED_CODEX_VERSION,
        TerminalState::noninteractive(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "active tasks require interactive restart approval"
    );
    assert_eq!(noninteractive_authority.requests().len(), 1);
    drop(noninteractive_authority);

    let declined_authority = ScriptedAuthority::start(
        &fixture.paths,
        vec![page(vec![active("thread-a", "Alpha")], None)],
    )
    .await;
    let (terminal, prompt) = TerminalState::scripted([""]);
    let error = baseline_active_turn_gate(&fixture.paths, TESTED_CODEX_VERSION, terminal)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "active task restart approval declined");
    assert_eq!(prompt.contents(), expected_prompt(&[("thread-a", "Alpha")]));
    assert_eq!(declined_authority.requests().len(), 1);
}

#[tokio::test]
async fn affirmative_rechecks_and_new_ids_require_the_complete_current_set_again() {
    let fixture = Fixture::new();
    let authority = ScriptedAuthority::start(
        &fixture.paths,
        vec![
            page(vec![active("thread-a", "Alpha")], None),
            page(
                vec![active("thread-a", "Alpha"), active("thread-b", "Beta")],
                None,
            ),
            page(
                vec![active("thread-a", "Alpha"), active("thread-b", "Beta")],
                None,
            ),
        ],
    )
    .await;
    let (terminal, prompt) = TerminalState::scripted(["y", "yes"]);

    baseline_active_turn_gate(&fixture.paths, TESTED_CODEX_VERSION, terminal)
        .await
        .unwrap();

    assert_eq!(authority.requests().len(), 3);
    assert_eq!(
        prompt.contents(),
        format!(
            "{}{}",
            expected_prompt(&[("thread-a", "Alpha")]),
            expected_prompt(&[("thread-a", "Alpha"), ("thread-b", "Beta")])
        )
    );
}

fn changed_update_context(fixture: &Fixture, candidate: PathBuf) -> (UpdateContext, PathBuf) {
    let new_bin = fixture.paths.home.join("new-codex-bin");
    fs::create_dir(&new_bin).unwrap();
    let new_codex = new_bin.join("codex");
    fs::copy(fixture.fake_bin.join("codex"), &new_codex).unwrap();
    fs::set_permissions(
        &new_codex,
        fs::metadata(fixture.fake_bin.join("codex"))
            .unwrap()
            .permissions(),
    )
    .unwrap();
    let setup = fixture.context(true);
    let mut path = vec![new_bin];
    path.extend(std::env::split_paths(&setup.path_environment));
    let (terminal, _) = TerminalState::scripted(["y"]);
    (
        UpdateContext {
            lifecycle: LifecycleContext {
                target: setup.target,
                path_environment: std::env::join_paths(path).unwrap(),
                desktop_environment: setup.desktop_environment,
                cwd: setup.cwd,
            },
            candidate,
            terminal,
        },
        new_codex,
    )
}

fn higher_candidate(fixture: &Fixture, name: &str) -> PathBuf {
    let candidate = fixture.paths.home.join(name);
    super::write_executable_fixture(
        &candidate,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'codex-session-control {} ({})\\n'; exit 0; fi\nexit 64\n",
            higher_test_release_version(),
            product_target()
        ),
    );
    candidate
}

#[tokio::test]
async fn every_gate_failure_keeps_installed_state_unchanged() {
    for case in ["initialize", "page", "decode", "noninteractive"] {
        let fixture = Fixture::new();
        let setup_authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        drop(setup_authority);
        let responses = match case {
            "initialize" => vec![],
            "page" => {
                vec![json!({"error": {"code": -32603, "message": "page failed"}})]
            }
            "decode" => vec![json!({"data": "not-an-array", "nextCursor": null})],
            "noninteractive" => {
                vec![page(vec![active("thread-a", "Alpha")], None)]
            }
            _ => unreachable!(),
        };
        let initialize_failures = if case == "initialize" {
            vec![false, true]
        } else {
            Vec::new()
        };
        let authority = ScriptedAuthority::start_with_initialize_failures(
            &fixture.paths,
            responses,
            initialize_failures,
        )
        .await;
        let candidate = higher_candidate(&fixture, &format!("candidate-{case}"));
        let (mut context, _) = changed_update_context(&fixture, candidate);
        if case == "noninteractive" {
            context.terminal = TerminalState::noninteractive();
        }
        let before_binary = fs::read(&fixture.paths.binary).unwrap();
        let before_manifest = fs::read(&fixture.paths.manifest).unwrap();
        fixture.clear_logs();

        let error = staged_update_with_context(context).await.unwrap_err();

        assert!(
            error.to_string().contains("failed at active-turn-gate:"),
            "{case}: {error}"
        );
        assert_eq!(fs::read(&fixture.paths.binary).unwrap(), before_binary);
        assert_eq!(fs::read(&fixture.paths.manifest).unwrap(), before_manifest);
        assert!(!fixture.systemctl_log().contains("daemon-reload"));
        assert!(!fixture.systemctl_log().contains(" restart "));
        drop(authority);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_gate_has_no_second_handoff_and_restarts_only_the_service() {
    let fixture = Fixture::new();
    let setup_authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    drop(setup_authority);
    let authority = ScriptedAuthority::start(
        &fixture.paths,
        vec![
            page(vec![active("thread-a", "Alpha")], None),
            page(vec![active("thread-a", "Alpha")], None),
        ],
    )
    .await;
    let candidate = higher_candidate(&fixture, "candidate-active-gate");
    let candidate_log = fixture.paths.home.join("candidate-active-gate.log");
    super::write_executable_fixture(
        &candidate,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = \"--version\" ]; then printf 'codex-session-control {} ({})\\n'; exit 0; fi\nexit 64\n",
            candidate_log.display(),
            higher_test_release_version(),
            product_target()
        ),
    );
    let (context, new_codex) = changed_update_context(&fixture, candidate);
    fs::write(&fixture.wait_for_socket, b"wait").unwrap();
    fixture.clear_logs();
    let paths = fixture.paths.clone();
    let restart_requested = fixture.restart_requested.clone();
    let starter = tokio::spawn(async move {
        while !restart_requested.exists() {
            tokio::task::yield_now().await;
        }
        FakeAuthority::start(&paths, TESTED_CODEX_VERSION).await
    });

    let report = staged_update_with_context(context).await.unwrap();
    let restarted_authority = starter.await.unwrap();

    assert!(report.stdout.starts_with(&format!(
        "Installed release: {}\n",
        higher_test_release_version()
    )));
    assert_eq!(
        serde_json::from_slice::<InstalledRelease>(&fs::read(&fixture.paths.manifest).unwrap())
            .unwrap()
            .codex_executable,
        new_codex
    );
    assert_eq!(authority.requests().len(), 2);
    assert_eq!(fixture.systemctl_log().matches(" restart ").count(), 1);
    assert!(!fixture.codex_log().contains("thread/interrupt"));
    assert_eq!(fs::read_to_string(candidate_log).unwrap(), "--version\n");
    drop(restarted_authority);
}
