mod app_server;
mod cli;
mod cli_output;
mod desktop;
mod diagnostics;
mod error;
mod install;
mod mcp;
mod model;
#[cfg(test)]
mod test_support;

use cli::{Cli, Command};
use cli_output::{OrdinaryFailure, RenderedCli, UserFailure};
use diagnostics::{DiagnosticCause, DiagnosticCommand, Diagnostics};
use error::ControllerError;
use std::io::{self, Write};

fn write_rendered(
    rendered: &RenderedCli,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> io::Result<()> {
    stdout.write_all(rendered.stdout.as_bytes())?;
    stdout.flush()?;
    stderr.write_all(rendered.stderr.as_bytes())?;
    stderr.flush()
}

enum ProcessOutcome {
    Render(RenderedCli),
    Exit(u8),
}

enum DispatchStage {
    Preflight,
}

impl DispatchStage {
    const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::Preflight => "preflight",
        }
    }
}

#[tokio::main]
async fn main() {
    match run(Cli::parse()).await {
        Ok(ProcessOutcome::Render(rendered)) => {
            let exit_code = rendered.exit_code;
            if write_rendered(&rendered, &mut io::stdout(), &mut io::stderr()).is_err() {
                std::process::exit(1);
            }
            if exit_code != 0 {
                std::process::exit(exit_code.into());
            }
        }
        Ok(ProcessOutcome::Exit(exit_code)) => {
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

async fn run(cli: Cli) -> Result<ProcessOutcome, ControllerError> {
    let verbose = cli.verbose;
    if matches!(&cli.command, Command::McpServer) {
        run_mcp_server().await?;
        return Ok(ProcessOutcome::Exit(0));
    }
    if let Command::Codex { args } = cli.command {
        install::codex_wrapper(args).await?;
        return Ok(ProcessOutcome::Exit(0));
    }
    if let Command::Setup { desktop_launcher } = cli.command {
        let mut diagnostics = Diagnostics::new(verbose, DiagnosticCommand::Setup);
        let result = install::setup(desktop_launcher.as_deref(), &mut diagnostics).await;
        diagnostics.flush();
        return Ok(ProcessOutcome::Render(match result {
            Ok(success) => success.render(),
            Err(failure) => failure.render(),
        }));
    }
    if matches!(&cli.command, Command::Update) {
        let mut diagnostics = Diagnostics::new(verbose, DiagnosticCommand::Update);
        let staged = match std::env::var_os("CODEX_SESSION_CONTROL_STAGED_UPDATE") {
            None => false,
            Some(value) if value == "1" => true,
            Some(_) => {
                diagnostics.failed(
                    DispatchStage::Preflight.diagnostic_name(),
                    DiagnosticCause::Unexpected,
                );
                diagnostics.flush();
                return Ok(ProcessOutcome::Render(
                    UserFailure::Ordinary(OrdinaryFailure::UpdateUnexpectedRetry).render(),
                ));
            }
        };
        let paths = match install::ResolvedUserPaths::from_effective_user() {
            Ok(paths) => paths,
            Err(_) => {
                diagnostics.failed(
                    DispatchStage::Preflight.diagnostic_name(),
                    DiagnosticCause::Unexpected,
                );
                diagnostics.flush();
                return Ok(ProcessOutcome::Render(
                    UserFailure::Ordinary(OrdinaryFailure::UpdateUnexpectedRetry).render(),
                ));
            }
        };
        let target = install::LifecycleTarget::production(paths);
        let result = install::update(target, staged, verbose, &mut diagnostics).await;
        diagnostics.flush();
        return Ok(match result {
            Ok(install::UpdateExecution::Render(success)) => {
                ProcessOutcome::Render(success.render())
            }
            Ok(install::UpdateExecution::PropagateCandidateExit(exit)) => {
                ProcessOutcome::Exit(exit.code())
            }
            Err(failure) => ProcessOutcome::Render(failure.render()),
        });
    }
    if matches!(&cli.command, Command::Status) {
        let paths = install::ResolvedUserPaths::from_effective_user()?;
        let target = install::LifecycleTarget::production(paths);
        let report = install::status(target).await?;
        print!("{}", report.stdout);
        return Ok(ProcessOutcome::Exit(if report.healthy { 0 } else { 1 }));
    }
    if matches!(&cli.command, Command::Enable | Command::Disable) {
        let command = match &cli.command {
            Command::Enable => DiagnosticCommand::Enable,
            Command::Disable => DiagnosticCommand::Disable,
            _ => unreachable!("guarded above"),
        };
        let mut diagnostics = Diagnostics::new(verbose, command);
        let result = match install::ResolvedUserPaths::from_effective_user() {
            Ok(paths) => {
                let target = install::LifecycleTarget::production(paths);
                match &cli.command {
                    Command::Enable => install::enable(target, &mut diagnostics).await,
                    Command::Disable => install::disable(target, &mut diagnostics).await,
                    _ => unreachable!("guarded above"),
                }
            }
            Err(_) => {
                diagnostics.failed(
                    DispatchStage::Preflight.diagnostic_name(),
                    DiagnosticCause::Validation,
                );
                Err(cli_output::UserFailure::Ordinary(match &cli.command {
                    Command::Enable => cli_output::OrdinaryFailure::EnableUnexpectedRetry,
                    Command::Disable => cli_output::OrdinaryFailure::DisableUnexpectedRetry,
                    _ => unreachable!("guarded above"),
                }))
            }
        };
        diagnostics.flush();
        return Ok(ProcessOutcome::Render(match result {
            Ok(success) => success.render(),
            Err(failure) => failure.render(),
        }));
    }
    if matches!(&cli.command, Command::Uninstall) {
        let mut diagnostics = Diagnostics::new(verbose, DiagnosticCommand::Uninstall);
        let result = match install::ResolvedUserPaths::from_effective_user() {
            Ok(paths) => {
                let target = install::LifecycleTarget::production(paths);
                install::uninstall(target, &mut diagnostics).await
            }
            Err(_) => {
                diagnostics.failed(
                    DispatchStage::Preflight.diagnostic_name(),
                    DiagnosticCause::Validation,
                );
                Err(cli_output::UserFailure::Ordinary(
                    cli_output::OrdinaryFailure::UninstallUnexpectedRetry,
                ))
            }
        };
        diagnostics.flush();
        return Ok(ProcessOutcome::Render(match result {
            Ok(success) => success.render(),
            Err(failure) => failure.render(),
        }));
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

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        io::{self, Write},
        rc::Rc,
    };

    use crate::cli_output::RenderedCli;

    use super::write_rendered;

    struct RecordingWriter {
        label: &'static str,
        events: Rc<RefCell<Vec<&'static str>>>,
        fail_write: bool,
    }

    impl Write for RecordingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail_write {
                return Err(io::Error::other("injected writer failure"));
            }
            self.events.borrow_mut().push(self.label);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.events.borrow_mut().push(match self.label {
                "stdout" => "flush-stdout",
                "stderr" => "flush-stderr",
                _ => unreachable!(),
            });
            Ok(())
        }
    }

    fn success_with_notice() -> RenderedCli {
        RenderedCli {
            stdout: "success\n".to_owned(),
            stderr: "notice\n".to_owned(),
            exit_code: 0,
        }
    }

    #[test]
    fn success_writer_flushes_stdout_before_default_visible_stderr() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut stdout = RecordingWriter {
            label: "stdout",
            events: Rc::clone(&events),
            fail_write: false,
        };
        let mut stderr = RecordingWriter {
            label: "stderr",
            events: Rc::clone(&events),
            fail_write: false,
        };

        write_rendered(&success_with_notice(), &mut stdout, &mut stderr).unwrap();

        assert_eq!(
            events.take(),
            ["stdout", "flush-stdout", "stderr", "flush-stderr"]
        );
    }

    #[test]
    fn writer_failure_exits_one_without_a_second_friendly_or_raw_error() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut stdout = RecordingWriter {
            label: "stdout",
            events: Rc::clone(&events),
            fail_write: true,
        };
        let mut stderr = RecordingWriter {
            label: "stderr",
            events: Rc::clone(&events),
            fail_write: false,
        };

        let exit_code = if write_rendered(&success_with_notice(), &mut stdout, &mut stderr).is_err()
        {
            1
        } else {
            0
        };

        assert_eq!(exit_code, 1);
        assert!(events.borrow().is_empty());
    }
}
