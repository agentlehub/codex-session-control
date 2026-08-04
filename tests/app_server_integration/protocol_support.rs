use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    error::Error,
    io,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UnixStream},
    sync::Notify,
};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

type NativeWebSocket = WebSocketStream<UnixStream>;

pub(super) struct NativeConnection {
    pub(super) websocket: NativeWebSocket,
    pub(super) next_id: u64,
}

impl NativeConnection {
    pub(super) async fn request(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, Box<dyn Error>> {
        let id = self.next_id;
        self.next_id += 1;
        self.websocket
            .send(Message::text(
                json!({"id": id, "method": method, "params": params}).to_string(),
            ))
            .await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let frame = self
                    .websocket
                    .next()
                    .await
                    .ok_or_else(|| io::Error::other("app-server disconnected"))?
                    .map_err(io::Error::other)?;
                let Message::Text(text) = frame else {
                    continue;
                };
                let value: Value = serde_json::from_str(text.as_str())?;
                if value.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = value.get("error") {
                    return Err(io::Error::other(format!("{method} failed: {error}")));
                }
                return value
                    .get("result")
                    .cloned()
                    .ok_or_else(|| io::Error::other(format!("{method} omitted result")));
            }
        })
        .await
        .map_err(|_| format!("{method} timed out"))?
        .map_err(Into::into)
    }
}

pub(super) struct ResponsesEndpoint {
    pub(super) address: std::net::SocketAddr,
    pub(super) requests: Arc<Mutex<Vec<Value>>>,
    errors: Arc<Mutex<Vec<String>>>,
    goal_canary: Arc<Mutex<Option<GoalCanary>>>,
    changed: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
struct GoalCanary {
    thread_id: String,
    objective: String,
}

impl ResponsesEndpoint {
    pub(super) async fn start() -> Result<Self, Box<dyn Error>> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        assert!(address.ip().is_loopback());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let errors = Arc::new(Mutex::new(Vec::new()));
        let goal_canary = Arc::new(Mutex::new(None));
        let changed = Arc::new(Notify::new());
        let served_requests = Arc::clone(&requests);
        let served_errors = Arc::clone(&errors);
        let served_goal_canary = Arc::clone(&goal_canary);
        let served_changed = Arc::clone(&changed);
        let task = tokio::spawn(async move {
            loop {
                let (stream, peer) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        served_errors.lock().unwrap().push(error.to_string());
                        return;
                    }
                };
                if !peer.ip().is_loopback() {
                    served_errors
                        .lock()
                        .unwrap()
                        .push(format!("rejected non-loopback peer {peer}"));
                    continue;
                }
                let requests = Arc::clone(&served_requests);
                let errors = Arc::clone(&served_errors);
                let goal_canary = Arc::clone(&served_goal_canary);
                let changed = Arc::clone(&served_changed);
                tokio::spawn(async move {
                    if let Err(error) =
                        serve_responses_request(stream, requests, goal_canary, changed).await
                    {
                        errors.lock().unwrap().push(error);
                    }
                });
            }
        });
        Ok(Self {
            address,
            requests,
            errors,
            goal_canary,
            changed,
            task,
        })
    }

    pub(super) fn prepare_goal_canary(&self, thread_id: &str, objective: &str) {
        *self.goal_canary.lock().unwrap() = Some(GoalCanary {
            thread_id: thread_id.to_owned(),
            objective: objective.to_owned(),
        });
    }

    pub(super) async fn wait_for_request(&self, expected: usize) -> Result<Value, Box<dyn Error>> {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Some(request) = self
                    .requests
                    .lock()
                    .unwrap()
                    .get(expected.saturating_sub(1))
                    .cloned()
                {
                    return request;
                }
                self.changed.notified().await;
            }
        })
        .await
        .map_err(|_| format!("Responses request {expected} did not arrive").into())
    }

    pub(super) async fn wait_for_requests(&self, expected: usize) -> Result<(), Box<dyn Error>> {
        self.wait_for_request(expected).await.map(|_| ())
    }

    pub(super) async fn wait_for_goal_output_after(
        &self,
        observed: usize,
    ) -> Result<Value, Box<dyn Error>> {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let matching = {
                    let requests = self.requests.lock().unwrap();
                    requests
                        .iter()
                        .skip(observed)
                        .find(|request| {
                            let outputs = request["input"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter(|item| {
                                    item["type"].as_str() == Some("function_call_output")
                                })
                                .collect::<Vec<_>>();
                            outputs.len() == 1
                                && outputs[0]["call_id"].as_str()
                                    == Some("call_session_control_goal")
                        })
                        .cloned()
                };
                if let Some(request) = matching {
                    return request;
                }
                self.changed.notified().await;
            }
        })
        .await
        .map_err(|_| "Responses goal tool output did not arrive".into())
    }

    pub(super) fn assert_clean(&self) -> Result<(), Box<dyn Error>> {
        let errors = self.errors.lock().unwrap();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!("loopback Responses endpoint errors: {errors:?}").into())
        }
    }
}

impl Drop for ResponsesEndpoint {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve_responses_request(
    mut stream: TcpStream,
    requests: Arc<Mutex<Vec<Value>>>,
    goal_canary: Arc<Mutex<Option<GoalCanary>>>,
    changed: Arc<Notify>,
) -> Result<(), String> {
    let (request_line, body) = read_http_request(&mut stream).await?;
    if request_line != "POST /v1/responses HTTP/1.1" {
        return Err(format!("unexpected loopback request: {request_line}"));
    }
    let request: Value = serde_json::from_slice(&body).map_err(|error| error.to_string())?;
    let hold = serde_json::to_string(&request)
        .map_err(|error| error.to_string())?
        .contains("HOLD_SESSION_CONTROL");
    let canary = goal_canary.lock().unwrap().clone();
    let canary_prompt =
        canary.is_some() && request.to_string().contains("SESSION_CONTROL_GOAL_CANARY");
    let canary_tool_result = request["input"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["type"].as_str() == Some("function_call_output")
                && item["call_id"].as_str() == Some("call_session_control_goal")
        })
    });
    requests.lock().unwrap().push(request);
    changed.notify_one();
    stream
        .write_all(
            concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: text/event-stream\r\n",
                "Cache-Control: no-cache\r\n",
                "Connection: close\r\n",
                "\r\n"
            )
            .as_bytes(),
        )
        .await
        .map_err(|error| error.to_string())?;
    if canary_tool_result {
        stream
            .write_all(include_bytes!("../fixtures/responses-completed.sse"))
            .await
            .map_err(|error| error.to_string())?;
        stream.shutdown().await.map_err(|error| error.to_string())
    } else if canary_prompt {
        let canary = canary.ok_or_else(|| "goal canary state was unavailable".to_owned())?;
        let arguments = json!({
            "threadId": canary.thread_id,
            "objective": canary.objective,
        })
        .to_string();
        let events = [
            json!({
                "type": "response.created",
                "response": {"id": "resp_session_control_goal"}
            }),
            json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "function_call",
                    "call_id": "call_session_control_goal",
                    "namespace": "mcp__codex_session_control",
                    "name": "thread_goal_set",
                    "arguments": arguments,
                }
            }),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_session_control_goal",
                    "usage": {
                        "input_tokens": 0,
                        "input_tokens_details": null,
                        "output_tokens": 0,
                        "output_tokens_details": null,
                        "total_tokens": 0,
                    }
                }
            }),
        ];
        for event in events {
            stream
                .write_all(
                    format!(
                        "event: {}\ndata: {event}\n\n",
                        event["type"].as_str().unwrap()
                    )
                    .as_bytes(),
                )
                .await
                .map_err(|error| error.to_string())?;
        }
        stream.shutdown().await.map_err(|error| error.to_string())
    } else if hold {
        let created = include_str!("../fixtures/responses-completed.sse")
            .split("\n\n")
            .next()
            .ok_or_else(|| "Responses fixture lacks response.created".to_owned())?;
        let mut created: Value = serde_json::from_str(
            created
                .strip_prefix("data: ")
                .ok_or_else(|| "Responses fixture response.created is malformed".to_owned())?,
        )
        .map_err(|error| error.to_string())?;
        created["response"]["id"] = json!("resp_session_control_held");
        stream
            .write_all(format!("data: {created}\n\n").as_bytes())
            .await
            .map_err(|error| error.to_string())?;
        stream.flush().await.map_err(|error| error.to_string())?;
        let mut byte = [0_u8; 1];
        match stream.read(&mut byte).await {
            Ok(0) => Ok(()),
            Ok(_) => Err("held Responses stream received client bytes".to_owned()),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset
                        | io::ErrorKind::BrokenPipe
                        | io::ErrorKind::UnexpectedEof
                ) =>
            {
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    } else {
        stream
            .write_all(include_bytes!("../fixtures/responses-completed.sse"))
            .await
            .map_err(|error| error.to_string())?;
        stream.shutdown().await.map_err(|error| error.to_string())
    }
}

async fn read_http_request(stream: &mut TcpStream) -> Result<(String, Vec<u8>), String> {
    let mut bytes = Vec::new();
    let header_end = loop {
        if bytes.len() > 1_048_576 {
            return Err("Responses request exceeded 1 MiB".to_owned());
        }
        let mut chunk = [0_u8; 4096];
        let count = stream
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("Responses request ended before headers".to_owned());
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end]).map_err(|error| error.to_string())?;
    let request_line = headers
        .lines()
        .next()
        .ok_or_else(|| "Responses request lacks request line".to_owned())?
        .to_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>())
            })
        })
        .transpose()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Responses request lacks content-length".to_owned())?;
    while bytes.len() < header_end + content_length {
        let mut chunk = [0_u8; 4096];
        let count = stream
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("Responses request body ended early".to_owned());
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok((
        request_line,
        bytes[header_end..header_end + content_length].to_vec(),
    ))
}
