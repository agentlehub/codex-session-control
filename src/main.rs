mod app_server;
mod error;
mod mcp;
mod model;
#[cfg(test)]
mod test_support;

#[tokio::main]
async fn main() {
    if std::env::args_os().len() != 1 {
        eprintln!("codex-session-control is a stdio MCP server and does not accept arguments");
        std::process::exit(2);
    }
    if let Err(error) = run_mcp_server().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run_mcp_server() -> Result<(), String> {
    let running = rmcp::serve_server(
        crate::mcp::SessionControlMcp::new(),
        rmcp::transport::stdio(),
    )
    .await
    .map_err(|error| error.to_string())?;
    running
        .waiting()
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}
