#![expect(
    clippy::result_large_err,
    reason = "native protocol stages return the approved structured ToolErrorData directly"
)]

use std::{future::Future, path::Path, time::Duration};

#[cfg(test)]
use std::path::PathBuf;

use futures_util::{SinkExt, StreamExt};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::net::UnixStream;
use tokio_tungstenite::{WebSocketStream, client_async, tungstenite::Message};

use crate::{
    error::{DispatchState, NativeErrorSummary, ToolErrorCategory, ToolErrorData},
    model::{Thread, ThreadGoal, ThreadSnapshot, Turn, TurnItemsView},
};

mod endpoint;
mod protocol;

use endpoint::DesktopEndpoint;
use protocol::{
    classify_native_error, compact_snapshot_from_native, protocol_fixture, thread_list_from_native,
    thread_read_from_native,
};

pub(crate) use protocol::{goal_from_native, thread_from_native, turn_from_native};

pub const NATIVE_STAGE_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const TESTED_CODEX_VERSION: &str = env!("CODEX_SESSION_CONTROL_TESTED_CODEX_VERSION");
#[cfg(test)]
pub(crate) const TESTED_CODEX_CLI_VERSION: &str = concat!(
    "codex-cli ",
    env!("CODEX_SESSION_CONTROL_TESTED_CODEX_VERSION")
);
#[cfg(test)]
pub(crate) const TESTED_CODEX_CLI_VERSION_OUTPUT: &str = concat!(
    "codex-cli ",
    env!("CODEX_SESSION_CONTROL_TESTED_CODEX_VERSION"),
    "\n"
);

type ClientWebSocket = WebSocketStream<UnixStream>;

#[derive(Clone, Debug)]
pub struct AppServerClient {
    endpoint_source: EndpointSource,
    product_version: String,
    tested_codex_version: String,
    #[cfg(test)]
    failure_point: FailurePoint,
}

#[derive(Debug)]
enum EndpointSource {
    Desktop,
    #[cfg(test)]
    Fixed(DesktopEndpoint),
}

impl Clone for EndpointSource {
    fn clone(&self) -> Self {
        match self {
            Self::Desktop => Self::Desktop,
            #[cfg(test)]
            Self::Fixed(endpoint) => Self::Fixed(DesktopEndpoint::explicit(
                endpoint.socket_path().to_path_buf(),
            )),
        }
    }
}

impl EndpointSource {
    fn resolve(&self) -> Result<DesktopEndpoint, ToolErrorData> {
        match self {
            Self::Desktop => DesktopEndpoint::resolve(),
            #[cfg(test)]
            Self::Fixed(endpoint) => Ok(DesktopEndpoint::explicit(
                endpoint.socket_path().to_path_buf(),
            )),
        }
    }
}

pub struct AppServerConnection {
    websocket: ClientWebSocket,
    next_request_id: u64,
    dispatch: MutationDispatch,
    compatibility_warning: Option<String>,
    #[cfg(test)]
    failure_point: FailurePoint,
    #[cfg(test)]
    initialize_result: Option<Value>,
}

impl std::fmt::Debug for AppServerConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppServerConnection")
            .field("next_request_id", &self.next_request_id)
            .field("dispatch", &self.dispatch)
            .field("compatibility_warning", &self.compatibility_warning)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationDispatch {
    NotDispatched,
    MayHaveBeenDispatched,
    CorrelatedResultReceived,
}

impl AppServerClient {
    fn new(
        endpoint_source: EndpointSource,
        product_version: &str,
        tested_codex_version: &str,
    ) -> Self {
        Self {
            endpoint_source,
            product_version: product_version.to_owned(),
            tested_codex_version: tested_codex_version.to_owned(),
            #[cfg(test)]
            failure_point: FailurePoint::Never,
        }
    }

    pub fn desktop() -> Self {
        Self::new(
            EndpointSource::Desktop,
            env!("CARGO_PKG_VERSION"),
            TESTED_CODEX_VERSION,
        )
    }

    #[cfg(test)]
    pub(crate) fn for_test(socket_path: PathBuf, tested_codex_version: &str) -> Self {
        Self::new(
            EndpointSource::Fixed(DesktopEndpoint::explicit(socket_path)),
            env!("CARGO_PKG_VERSION"),
            tested_codex_version,
        )
    }

    pub async fn connect_initialized(&self) -> Result<AppServerConnection, ToolErrorData> {
        let endpoint = self.endpoint_source.resolve()?;
        endpoint.validate()?;

        let (websocket, _) = with_native_stage_timeout("connect", async {
            let stream = UnixStream::connect(endpoint.socket_path())
                .await
                .map_err(|_| transport_error("connect"))?;
            client_async("ws://localhost/rpc", stream)
                .await
                .map_err(|_| transport_error("connect"))
        })
        .await?;
        let mut connection = AppServerConnection::new(websocket);
        #[cfg(test)]
        {
            connection.failure_point = self.failure_point;
        }
        with_native_stage_timeout("initialize", async {
            connection
                .initialize(&self.product_version, &self.tested_codex_version)
                .await
        })
        .await?;

        Ok(connection)
    }
}

impl AppServerConnection {
    fn new(websocket: ClientWebSocket) -> Self {
        Self {
            websocket,
            next_request_id: 1,
            dispatch: MutationDispatch::NotDispatched,
            compatibility_warning: None,
            #[cfg(test)]
            failure_point: FailurePoint::Never,
            #[cfg(test)]
            initialize_result: None,
        }
    }

    async fn initialize(
        &mut self,
        product_version: &str,
        tested_codex_version: &str,
    ) -> Result<(), ToolErrorData> {
        let initialize_result: Value = self
            .request_with_timeout(
                "initialize",
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "codex_session_control",
                        "title": "Codex Session Control",
                        "version": product_version,
                    },
                    "capabilities": {
                        "experimentalApi": true,
                        "mcpServerOpenaiFormElicitation": false,
                        "requestAttestation": false,
                        "optOutNotificationMethods": [],
                    }
                }),
                false,
                (None, None),
                false,
            )
            .await?;

        let valid_codex_home = initialize_result
            .get("codexHome")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty() && Path::new(value).is_absolute());
        if !valid_codex_home {
            return Err(ToolErrorData::fixed(
                ToolErrorCategory::TargetUnavailable,
                "initialize",
                "initialize",
            ));
        }

        let reported_version = initialize_result
            .get("userAgent")
            .and_then(Value::as_str)
            .and_then(extract_codex_version)
            .unwrap_or_else(|| "unknown".to_owned());
        if reported_version != tested_codex_version {
            self.compatibility_warning = Some(format!(
                "WARNING: Target Codex {reported_version} is untested. Codex session control was validated against Codex {tested_codex_version}. Report this warning to the operator. The accompanying structured data remains authoritative."
            ));
        }
        #[cfg(test)]
        {
            self.initialize_result = Some(initialize_result);
        }

        self.websocket
            .send(Message::text(json!({"method": "initialized"}).to_string()))
            .await
            .map_err(|_| transport_error("initialize"))?;
        Ok(())
    }

    pub async fn request<R: DeserializeOwned>(
        &mut self,
        method: &'static str,
        params: impl Serialize,
    ) -> Result<R, ToolErrorData> {
        self.request_with_timeout(method, method, params, false, (None, None), true)
            .await
    }

    pub async fn mutate<R: DeserializeOwned>(
        &mut self,
        tool: &'static str,
        method: &'static str,
        params: impl Serialize,
        known_thread_id: Option<&str>,
        known_turn_id: Option<&str>,
    ) -> Result<R, ToolErrorData> {
        self.request_with_timeout(
            tool,
            method,
            params,
            true,
            (known_thread_id, known_turn_id),
            true,
        )
        .await
    }

    pub async fn compact_snapshot(
        &mut self,
        thread_id: &str,
    ) -> Result<ThreadSnapshot, ToolErrorData> {
        let metadata: Value = self
            .request(
                "thread/read",
                json!({"threadId": thread_id, "includeTurns": false}),
            )
            .await?;
        let latest: Value = self
            .request(
                "thread/turns/list",
                json!({
                    "threadId": thread_id,
                    "limit": 1,
                    "itemsView": "notLoaded",
                }),
            )
            .await?;
        compact_snapshot_from_native(thread_id, &metadata, &latest)
    }

    pub async fn threads_list(
        &mut self,
        cursor: Option<&str>,
        limit: Option<u32>,
        archived: Option<bool>,
        cwd: Option<&str>,
    ) -> Result<(Vec<Thread>, Option<String>), ToolErrorData> {
        let mut params = serde_json::Map::new();
        insert_optional(&mut params, "cursor", cursor)?;
        insert_optional(&mut params, "limit", limit)?;
        insert_optional(&mut params, "archived", archived)?;
        insert_optional(&mut params, "cwd", cwd)?;
        let response: Value = self.request("thread/list", params).await?;
        thread_list_from_native(&response)
    }

    pub(crate) async fn spawned_descendants_page(
        &mut self,
        ancestor_thread_id: &str,
        cursor: Option<&str>,
    ) -> Result<(Vec<Thread>, Option<String>), ToolErrorData> {
        let mut params = serde_json::Map::new();
        params.insert("ancestorThreadId".to_owned(), json!(ancestor_thread_id));
        params.insert("sourceKinds".to_owned(), json!(["subAgentThreadSpawn"]));
        insert_optional(&mut params, "cursor", cursor)?;
        let response: Value = self.request("thread/list", params).await?;
        thread_list_from_native(&response)
    }

    pub async fn threads_list_for_update(
        &mut self,
        cursor: Option<&str>,
    ) -> Result<(Vec<Thread>, Option<String>), ToolErrorData> {
        let mut params = serde_json::Map::new();
        insert_optional(&mut params, "cursor", cursor)?;
        params.insert("archived".to_owned(), Value::Bool(false));
        params.insert(
            "sourceKinds".to_owned(),
            json!([
                "cli",
                "vscode",
                "exec",
                "appServer",
                "subAgent",
                "subAgentReview",
                "subAgentCompact",
                "subAgentThreadSpawn",
                "subAgentOther",
                "unknown"
            ]),
        );
        let response: Value = self.request("thread/list", params).await?;
        thread_list_from_native(&response)
    }

    pub async fn thread_read(
        &mut self,
        thread_id: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
        items_view: Option<TurnItemsView>,
    ) -> Result<(Thread, Vec<Turn>, Option<String>), ToolErrorData> {
        let metadata: Value = self
            .request(
                "thread/read",
                json!({"threadId": thread_id, "includeTurns": false}),
            )
            .await?;
        let mut params = serde_json::Map::new();
        params.insert("threadId".to_owned(), Value::String(thread_id.to_owned()));
        insert_optional(&mut params, "cursor", cursor)?;
        insert_optional(&mut params, "limit", limit)?;
        insert_optional(&mut params, "itemsView", items_view)?;
        let turns: Value = self.request("thread/turns/list", params).await?;
        thread_read_from_native(&metadata, &turns)
    }

    pub async fn thread_goal_get(
        &mut self,
        thread_id: &str,
    ) -> Result<Option<ThreadGoal>, ToolErrorData> {
        let response: Value = self
            .request("thread/goal/get", json!({"threadId": thread_id}))
            .await?;
        goal_from_native(&response, thread_id, "thread/goal/get")
    }

    pub async fn wait_for_notification_or_quiet(&mut self) -> Result<(), ToolErrorData> {
        match tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let message = self
                    .websocket
                    .next()
                    .await
                    .ok_or_else(|| transport_error("observe"))?
                    .map_err(|_| transport_error("observe"))?;
                let Message::Text(text) = message else {
                    continue;
                };
                let value: Value =
                    serde_json::from_str(&text).map_err(|_| transport_error("observe"))?;
                if value.get("method").is_some() && value.get("id").is_none() {
                    return Ok(());
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Ok(()),
        }
    }

    pub fn compatibility_warning(&self) -> Option<&str> {
        self.compatibility_warning.as_deref()
    }

    async fn request_with_timeout<R: DeserializeOwned>(
        &mut self,
        tool: &'static str,
        method: &'static str,
        params: impl Serialize,
        mutation: bool,
        known_ids: (Option<&str>, Option<&str>),
        apply_stage_timeout: bool,
    ) -> Result<R, ToolErrorData> {
        let (known_thread_id, known_turn_id) = known_ids;
        let params = serde_json::to_value(params)
            .map_err(|_| invalid_request(tool, method, known_thread_id, known_turn_id))?;
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        let request = json!({
            "id": request_id,
            "method": method,
            "params": params,
        });

        #[cfg(test)]
        if mutation && self.failure_point == FailurePoint::BeforeWrite {
            return Err(request_transport_error(
                tool,
                method,
                true,
                self.dispatch,
                known_thread_id,
                known_turn_id,
            ));
        }
        if mutation {
            self.dispatch = MutationDispatch::MayHaveBeenDispatched;
        }
        #[cfg(test)]
        if mutation && self.failure_point == FailurePoint::AfterPartialWrite {
            return Err(request_transport_error(
                tool,
                method,
                true,
                self.dispatch,
                known_thread_id,
                known_turn_id,
            ));
        }

        let operation = async {
            self.websocket
                .send(Message::text(request.to_string()))
                .await
                .map_err(|_| {
                    request_transport_error(
                        tool,
                        method,
                        mutation,
                        self.dispatch,
                        known_thread_id,
                        known_turn_id,
                    )
                })?;
            #[cfg(test)]
            if mutation && self.failure_point == FailurePoint::AfterFullWrite {
                return Err(request_transport_error(
                    tool,
                    method,
                    true,
                    self.dispatch,
                    known_thread_id,
                    known_turn_id,
                ));
            }

            loop {
                let frame = self.websocket.next().await.ok_or_else(|| {
                    request_transport_error(
                        tool,
                        method,
                        mutation,
                        self.dispatch,
                        known_thread_id,
                        known_turn_id,
                    )
                })?;
                let message = frame.map_err(|_| {
                    request_transport_error(
                        tool,
                        method,
                        mutation,
                        self.dispatch,
                        known_thread_id,
                        known_turn_id,
                    )
                })?;
                let Message::Text(text) = message else {
                    if message.is_close() {
                        return Err(request_transport_error(
                            tool,
                            method,
                            mutation,
                            self.dispatch,
                            known_thread_id,
                            known_turn_id,
                        ));
                    }
                    continue;
                };
                let value: Value = serde_json::from_str(text.as_str()).map_err(|_| {
                    request_transport_error(
                        tool,
                        method,
                        mutation,
                        self.dispatch,
                        known_thread_id,
                        known_turn_id,
                    )
                })?;
                if value.get("method").is_some() {
                    continue;
                }
                if value.get("id").and_then(Value::as_u64) != Some(request_id) {
                    continue;
                }
                if mutation {
                    self.dispatch = MutationDispatch::CorrelatedResultReceived;
                }
                if let Some(error) = value.get("error") {
                    return Err(native_error(
                        tool,
                        method,
                        error,
                        mutation,
                        known_thread_id,
                        known_turn_id,
                    ));
                }
                let result = value.get("result").cloned().ok_or_else(|| {
                    request_transport_error(
                        tool,
                        method,
                        mutation,
                        self.dispatch,
                        known_thread_id,
                        known_turn_id,
                    )
                })?;
                return serde_json::from_value(result).map_err(|_| {
                    request_transport_error(
                        tool,
                        method,
                        mutation,
                        self.dispatch,
                        known_thread_id,
                        known_turn_id,
                    )
                });
            }
        };
        let result = if apply_stage_timeout {
            with_native_stage_timeout(method, operation).await
        } else {
            operation.await
        };

        match result {
            Ok(result) => {
                if mutation {
                    self.dispatch = MutationDispatch::CorrelatedResultReceived;
                }
                Ok(result)
            }
            Err(mut error) if mutation && error.category == ToolErrorCategory::StageTimeout => {
                error.category = ToolErrorCategory::OutcomeUnknown;
                error.message =
                    "Mutation outcome is unknown. The request may already have been applied."
                        .to_owned();
                error.tool = tool.to_owned();
                error.thread_id = known_thread_id.map(ToOwned::to_owned);
                error.turn_id = known_turn_id.map(ToOwned::to_owned);
                error.dispatch = Some(DispatchState::MayHaveBeenDispatched);
                Err(error)
            }
            Err(error) => Err(error),
        }
    }
}

pub async fn with_native_stage_timeout<T>(
    stage: &'static str,
    future: impl Future<Output = Result<T, ToolErrorData>>,
) -> Result<T, ToolErrorData> {
    tokio::time::timeout(NATIVE_STAGE_TIMEOUT, future)
        .await
        .map_err(|_| ToolErrorData::fixed(ToolErrorCategory::StageTimeout, stage, stage))?
}

fn extract_codex_version(user_agent: &str) -> Option<String> {
    user_agent
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '+'))
        })
        .find_map(|candidate| {
            semver::Version::parse(candidate)
                .ok()
                .map(|version| version.to_string())
        })
}

fn insert_optional<T: Serialize>(
    params: &mut serde_json::Map<String, Value>,
    name: &'static str,
    value: Option<T>,
) -> Result<(), ToolErrorData> {
    if let Some(value) = value {
        params.insert(
            name.to_owned(),
            serde_json::to_value(value).map_err(|_| invalid_request(name, name, None, None))?,
        );
    }
    Ok(())
}

fn invalid_request(
    tool: &str,
    stage: &str,
    thread_id: Option<&str>,
    turn_id: Option<&str>,
) -> ToolErrorData {
    let mut error = ToolErrorData::fixed(ToolErrorCategory::InvalidRequest, tool, stage);
    error.thread_id = thread_id.map(ToOwned::to_owned);
    error.turn_id = turn_id.map(ToOwned::to_owned);
    error
}

fn transport_error(stage: &str) -> ToolErrorData {
    ToolErrorData::fixed(
        ToolErrorCategory::AuthorityTransportFailure,
        "native_transport",
        stage,
    )
}

fn request_transport_error(
    tool: &str,
    stage: &str,
    mutation: bool,
    dispatch: MutationDispatch,
    thread_id: Option<&str>,
    turn_id: Option<&str>,
) -> ToolErrorData {
    let category = if mutation && dispatch == MutationDispatch::MayHaveBeenDispatched {
        ToolErrorCategory::OutcomeUnknown
    } else {
        ToolErrorCategory::AuthorityTransportFailure
    };
    let mut error = ToolErrorData::fixed(category, tool, stage);
    error.thread_id = thread_id.map(ToOwned::to_owned);
    error.turn_id = turn_id.map(ToOwned::to_owned);
    if mutation {
        error.dispatch = Some(match dispatch {
            MutationDispatch::NotDispatched => DispatchState::NotDispatched,
            MutationDispatch::MayHaveBeenDispatched
            | MutationDispatch::CorrelatedResultReceived => DispatchState::MayHaveBeenDispatched,
        });
    }
    error
}

fn native_error(
    tool: &str,
    method: &str,
    value: &Value,
    mutation: bool,
    thread_id: Option<&str>,
    turn_id: Option<&str>,
) -> ToolErrorData {
    let code = value.get("code").and_then(Value::as_i64);
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("native app-server request failed");
    let category = code.map_or(ToolErrorCategory::NativeError, |code| {
        classify_native_error(method, code, message, value.get("data"), protocol_fixture())
    });
    let mut error = ToolErrorData::fixed(category, tool, method);
    error.native = Some(NativeErrorSummary {
        code,
        message: message.to_owned(),
    });
    error.thread_id = thread_id.map(ToOwned::to_owned);
    error.turn_id = turn_id.map(ToOwned::to_owned);
    if mutation {
        error.dispatch = Some(DispatchState::MayHaveBeenDispatched);
    }
    error
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailurePoint {
    Never,
    BeforeWrite,
    AfterPartialWrite,
    AfterFullWrite,
    BeforeResponse,
    AfterNativeStateChange,
}

#[cfg(test)]
mod tests;
