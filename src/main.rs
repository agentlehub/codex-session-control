mod app_server;
mod cli;
mod desktop;
mod error;
mod install;
mod mcp;
mod model;
#[cfg(test)]
mod test_support;

use clap::Parser;
use cli::{Cli, Command};
use error::ControllerError;

#[tokio::main]
async fn main() {
    match run(Cli::parse()).await {
        Ok(exit_code) => {
            if exit_code != 0 {
                std::process::exit(exit_code.into());
            }
        }
        Err(error) => {
            let rendered = error.to_string();
            eprint!("{rendered}");
            if !rendered.ends_with('\n') {
                eprintln!();
            }
            std::process::exit(error.exit_code().into());
        }
    }
}

async fn run(cli: Cli) -> Result<u8, ControllerError> {
    if matches!(&cli.command, Command::McpServer) {
        run_mcp_server().await?;
        return Ok(0);
    }
    if let Command::Codex { args } = cli.command {
        install::codex_wrapper(args).await?;
        return Ok(0);
    }
    if let Command::Setup { desktop_launcher } = cli.command {
        let report = install::setup(desktop_launcher.as_deref()).await?;
        eprint!("{}", report.stderr);
        print!("{}", report.stdout);
        return Ok(0);
    }
    if matches!(&cli.command, Command::Update) {
        let staged = match std::env::var_os("CODEX_SESSION_CONTROL_STAGED_UPDATE") {
            None => false,
            Some(value) if value == "1" => true,
            Some(_) => {
                return Err(ControllerError::Operational(
                    "candidate-preflight rejected invalid staged update marker".to_owned(),
                ));
            }
        };
        let paths = install::ResolvedUserPaths::from_effective_user()?;
        let target = install::LifecycleTarget::production(paths);
        let report = install::update(target, staged).await?;
        eprint!("{}", report.stderr);
        print!("{}", report.stdout);
        return Ok(0);
    }
    if matches!(&cli.command, Command::Status) {
        let paths = install::ResolvedUserPaths::from_effective_user()?;
        let target = install::LifecycleTarget::production(paths);
        let report = install::status(target).await?;
        print!("{}", report.stdout);
        return Ok(if report.healthy { 0 } else { 1 });
    }
    if matches!(&cli.command, Command::Enable | Command::Disable) {
        let paths = install::ResolvedUserPaths::from_effective_user()?;
        let target = install::LifecycleTarget::production(paths);
        let report = match &cli.command {
            Command::Enable => install::enable(target).await?,
            Command::Disable => install::disable(target).await?,
            _ => unreachable!("guarded above"),
        };
        eprint!("{}", report.stderr);
        print!("{}", report.stdout);
        return Ok(0);
    }
    if matches!(&cli.command, Command::Uninstall) {
        let paths = install::ResolvedUserPaths::from_effective_user()?;
        let target = install::LifecycleTarget::production(paths);
        let report = install::uninstall(target).await?;
        eprint!("{}", report.stderr);
        print!("{}", report.stdout);
        return Ok(0);
    }

    unreachable!("all command variants are handled above")
}

async fn run_mcp_server() -> Result<(), ControllerError> {
    let service = crate::mcp::SessionControlMcp::new();
    let running = rmcp::serve_server(service, rmcp::transport::stdio())
        .await
        .map_err(|error| ControllerError::Operational(error.to_string()))?;
    running
        .waiting()
        .await
        .map(|_| ())
        .map_err(|error| ControllerError::Operational(error.to_string()))
}
