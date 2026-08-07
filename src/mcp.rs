#![expect(
    clippy::result_large_err,
    reason = "the approved validation boundary returns structured ToolErrorData directly"
)]

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResult, ContentBlock, ErrorCode, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
};
use serde_json::Value;

use crate::{
    app_server::AppServerClient,
    error::{ToolErrorCategory, ToolErrorData},
    install::load_installed_config,
    model::ProductConfig,
};

mod contract;
mod operations;
mod wait;

#[cfg(test)]
mod tests;

use contract::*;
use operations::*;
use wait::*;

pub struct SessionControlMcp;

impl SessionControlMcp {
    pub fn new() -> Self {
        Self
    }
}

impl ServerHandler for SessionControlMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(catalog()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let arguments = request.arguments.unwrap_or_default();
        let meta = Value::Object(context.meta.0);
        let validated = validate_input(request.name.as_ref(), arguments, &meta)
            .map_err(|error| error_response(error, None))?;
        execute_validated(request.name.as_ref(), validated).await
    }
}

async fn execute_validated(
    tool: &str,
    validated: ValidatedInput,
) -> Result<CallToolResult, McpError> {
    let config = load_installed_config().map_err(|_| {
        error_response(
            ToolErrorData::fixed(ToolErrorCategory::TargetUnavailable, tool, "configuration"),
            None,
        )
    })?;
    execute_tool(tool, validated, &config).await
}

async fn execute_tool(
    tool: &str,
    validated: ValidatedInput,
    config: &ProductConfig,
) -> Result<CallToolResult, McpError> {
    let client = AppServerClient::from_config(config);
    let mut connection = client
        .connect_initialized()
        .await
        .map_err(|error| response_error(tool, error, None))?;
    let warning = connection.compatibility_warning().map(str::to_owned);
    let structured = match validated {
        ValidatedInput::ThreadsList(input) => {
            let (threads, next_cursor) = connection
                .threads_list(
                    input.cursor.as_deref(),
                    input.limit,
                    input.archived,
                    input.cwd.as_deref(),
                )
                .await
                .map_err(|error| response_error(tool, error, warning.as_deref()))?;
            serde_json::to_value(ThreadsListResult {
                threads,
                next_cursor,
            })
        }
        ValidatedInput::ThreadRead(input) => {
            let (thread, turns, next_cursor) = connection
                .thread_read(
                    &input.thread_id,
                    input.cursor.as_deref(),
                    input.limit,
                    input.items_view,
                )
                .await
                .map_err(|error| response_error(tool, error, warning.as_deref()))?;
            serde_json::to_value(ThreadReadResult {
                thread,
                turns,
                next_cursor,
            })
        }
        ValidatedInput::ThreadGoalGet(input) => {
            let goal = connection
                .thread_goal_get(&input.thread_id)
                .await
                .map_err(|error| response_error(tool, error, warning.as_deref()))?;
            serde_json::to_value(ThreadGoalGetResult { goal })
        }
        ValidatedInput::ThreadsWait { input, timeout } => {
            let result = threads_wait(&mut connection, &input.thread_ids, timeout)
                .await
                .map_err(|error| response_error(tool, error, warning.as_deref()))?;
            serde_json::to_value(result)
        }
        ValidatedInput::ThreadCreate(input) => serde_json::to_value(
            create_thread(&client, &mut connection, input)
                .await
                .map_err(|error| response_error(tool, error, warning.as_deref()))?,
        ),
        ValidatedInput::ThreadFork(input) => serde_json::to_value(
            fork_thread(&client, &mut connection, input)
                .await
                .map_err(|error| response_error(tool, error, warning.as_deref()))?,
        ),
        ValidatedInput::ThreadMessageSend(input) => serde_json::to_value(
            send_message(&client, &mut connection, input)
                .await
                .map_err(|error| response_error(tool, error, warning.as_deref()))?,
        ),
        ValidatedInput::ThreadTitleSet(input) => serde_json::to_value(
            set_title(&client, &mut connection, input)
                .await
                .map_err(|error| response_error(tool, error, warning.as_deref()))?,
        ),
        ValidatedInput::ThreadPinSet(input) => serde_json::to_value(
            set_pin(&client, &mut connection, input)
                .await
                .map_err(|error| response_error(tool, error, warning.as_deref()))?,
        ),
        ValidatedInput::ThreadGoalSet(input) => {
            let goal = set_goal(&client, &mut connection, input)
                .await
                .map_err(|error| response_error(tool, error, warning.as_deref()))?;
            serde_json::to_value(ThreadGoalSetResult { goal })
        }
        ValidatedInput::ThreadGoalPause(input) => {
            let goal = pause_goal(&client, &mut connection, input)
                .await
                .map_err(|error| response_error(tool, error, warning.as_deref()))?;
            serde_json::to_value(ThreadGoalPauseResult { goal })
        }
        ValidatedInput::ThreadGoalResume(input) => {
            let goal = resume_goal(&client, &mut connection, input)
                .await
                .map_err(|error| response_error(tool, error, warning.as_deref()))?;
            serde_json::to_value(ThreadGoalResumeResult { goal })
        }
        ValidatedInput::ThreadGoalClear(input) => serde_json::to_value(
            clear_goal(&client, &mut connection, input)
                .await
                .map_err(|error| response_error(tool, error, warning.as_deref()))?,
        ),
        ValidatedInput::ThreadInterrupt(input) => serde_json::to_value(
            interrupt_thread(&client, &mut connection, input)
                .await
                .map_err(|error| response_error(tool, error, warning.as_deref()))?,
        ),
    }
    .map_err(|_| {
        error_response(
            ToolErrorData::fixed(ToolErrorCategory::NativeError, tool, "serialize"),
            warning.as_deref(),
        )
    })?;
    Ok(success_result(structured, warning.as_deref()))
}

fn success_result(structured: Value, warning: Option<&str>) -> CallToolResult {
    let text = serde_json::to_string(&structured).expect("JSON values always serialize");
    let mut result = CallToolResult::success(vec![ContentBlock::text(prefix_text(warning, &text))]);
    result.structured_content = Some(structured);
    result
}

fn error_response(error: ToolErrorData, warning: Option<&str>) -> McpError {
    let code = if error.category == ToolErrorCategory::InvalidRequest {
        ErrorCode::INVALID_PARAMS
    } else {
        ErrorCode(-32000)
    };
    let message = prefix_text(warning, &error.message);
    let data = serde_json::to_value(error).expect("tool errors always serialize");
    McpError::new(code, message, Some(data))
}

fn response_error(tool: &str, mut error: ToolErrorData, warning: Option<&str>) -> McpError {
    error.tool = tool.to_owned();
    error_response(error, warning)
}

fn prefix_text(warning: Option<&str>, text: &str) -> String {
    match warning {
        Some(warning) => format!("{warning}\n\n{text}"),
        None => text.to_owned(),
    }
}
