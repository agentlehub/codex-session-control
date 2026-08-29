#[allow(
    dead_code,
    reason = "Task 11 owns deletion of the unreachable lifecycle-only runtime surface"
)]
mod app_server;
#[allow(
    dead_code,
    reason = "Task 11 owns deletion of the unreachable lifecycle-only error surface"
)]
mod error;
mod mcp;
#[allow(
    dead_code,
    reason = "Task 11 owns deletion of the unreachable lifecycle-only models"
)]
mod model;
#[cfg(test)]
mod test_support;

#[tokio::main]
async fn main() {
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
