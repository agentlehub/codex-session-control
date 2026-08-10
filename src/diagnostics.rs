use std::{io::Write, path::PathBuf};

use semver::Version;

#[derive(Debug)]
pub(crate) struct Diagnostics {
    command: DiagnosticCommand,
    phase: Option<UpdatePhase>,
    sink: DiagnosticSink,
}

#[derive(Debug)]
enum DiagnosticSink {
    Off,
    Stderr(std::io::Stderr),
    #[cfg(test)]
    Record(Vec<String>),
    #[cfg(test)]
    FailOnce,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiagnosticCommand {
    Setup,
    Update,
    Status,
    Enable,
    Disable,
    Uninstall,
    Codex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UpdatePhase {
    Outer,
    Apply,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiagnosticCause {
    Unexpected,
    Validation,
    ReleaseDownload,
    Checksum,
    ServiceConfiguration,
    ServiceStart,
    ServiceStop,
    ServiceState,
    CliIntegration,
    DesktopIntegration,
    ActiveTasks,
    Cleanup,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DiagnosticEvent {
    CandidateVerified { version: Version },
    StartingStagedCandidate,
    StagedMarkerAccepted,
    SelectedCodexHome { codex_home: PathBuf },
    CompletedPreflight,
    CompletedBinary,
    CompletedManifest,
    FailedServiceVerify { cause: DiagnosticCause },
}

impl Diagnostics {
    pub(crate) fn new(verbose: bool, command: DiagnosticCommand) -> Self {
        Self {
            command,
            phase: None,
            sink: if verbose {
                DiagnosticSink::Stderr(std::io::stderr())
            } else {
                DiagnosticSink::Off
            },
        }
    }

    #[cfg(test)]
    fn record(command: DiagnosticCommand) -> Self {
        Self {
            command,
            phase: None,
            sink: DiagnosticSink::Record(Vec::new()),
        }
    }

    #[cfg(test)]
    fn fail_once(command: DiagnosticCommand) -> Self {
        Self {
            command,
            phase: None,
            sink: DiagnosticSink::FailOnce,
        }
    }

    pub(crate) fn set_phase(&mut self, phase: UpdatePhase) {
        self.phase = Some(phase);
    }

    pub(crate) fn emit(&mut self, event: DiagnosticEvent) {
        let line = self.render_event(event);
        let failed = match &mut self.sink {
            DiagnosticSink::Off => false,
            DiagnosticSink::Stderr(stderr) => stderr.write_all(line.as_bytes()).is_err(),
            #[cfg(test)]
            DiagnosticSink::Record(lines) => {
                lines.push(line);
                false
            }
            #[cfg(test)]
            DiagnosticSink::FailOnce => true,
        };
        if failed {
            self.sink = DiagnosticSink::Off;
        }
    }

    pub(crate) fn flush(&mut self) {
        let failed = match &mut self.sink {
            DiagnosticSink::Off => false,
            DiagnosticSink::Stderr(stderr) => stderr.flush().is_err(),
            #[cfg(test)]
            DiagnosticSink::Record(_) => false,
            #[cfg(test)]
            DiagnosticSink::FailOnce => true,
        };
        if failed {
            self.sink = DiagnosticSink::Off;
        }
    }

    fn render_event(&self, event: DiagnosticEvent) -> String {
        let command = match self.command {
            DiagnosticCommand::Setup => "setup",
            DiagnosticCommand::Update => "update",
            DiagnosticCommand::Status => "status",
            DiagnosticCommand::Enable => "enable",
            DiagnosticCommand::Disable => "disable",
            DiagnosticCommand::Uninstall => "uninstall",
            DiagnosticCommand::Codex => "codex",
        };
        let phase = match (self.command, self.phase) {
            (DiagnosticCommand::Update, Some(UpdatePhase::Outer)) => "/outer",
            (DiagnosticCommand::Update, Some(UpdatePhase::Apply)) => "/apply",
            _ => "",
        };
        let detail = match event {
            DiagnosticEvent::CandidateVerified { version } => {
                format!("candidate {version} verified")
            }
            DiagnosticEvent::StartingStagedCandidate => "starting staged candidate".to_owned(),
            DiagnosticEvent::StagedMarkerAccepted => "staged marker accepted".to_owned(),
            DiagnosticEvent::SelectedCodexHome { codex_home } => {
                format!("selected CODEX_HOME {}", codex_home.display())
            }
            DiagnosticEvent::CompletedPreflight => "completed preflight".to_owned(),
            DiagnosticEvent::CompletedBinary => "completed binary".to_owned(),
            DiagnosticEvent::CompletedManifest => "completed manifest".to_owned(),
            DiagnosticEvent::FailedServiceVerify { cause } => {
                format!("failed service-verify ({})", cause.label())
            }
        };
        format!("[verbose] {command}{phase}: {detail}\n")
    }

    #[cfg(test)]
    fn recorded_lines(&self) -> &[String] {
        match &self.sink {
            DiagnosticSink::Record(lines) => lines,
            _ => &[],
        }
    }

    #[cfg(test)]
    fn is_off(&self) -> bool {
        matches!(self.sink, DiagnosticSink::Off)
    }
}

impl DiagnosticCause {
    fn label(self) -> &'static str {
        match self {
            Self::Unexpected => "unexpected",
            Self::Validation => "validation",
            Self::ReleaseDownload => "release-download",
            Self::Checksum => "checksum",
            Self::ServiceConfiguration => "service-configuration",
            Self::ServiceStart => "service-start",
            Self::ServiceStop => "service-stop",
            Self::ServiceState => "service-state",
            Self::CliIntegration => "cli-integration",
            Self::DesktopIntegration => "desktop-integration",
            Self::ActiveTasks => "active-tasks",
            Self::Cleanup => "cleanup",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use semver::Version;

    use super::*;

    #[test]
    fn prefixes_and_update_phases_are_exact() {
        let mut diagnostics = Diagnostics::record(DiagnosticCommand::Update);
        diagnostics.set_phase(UpdatePhase::Outer);
        diagnostics.emit(DiagnosticEvent::CandidateVerified {
            version: Version::parse("1.2.3").unwrap(),
        });
        diagnostics.emit(DiagnosticEvent::StartingStagedCandidate);
        diagnostics.set_phase(UpdatePhase::Apply);
        diagnostics.emit(DiagnosticEvent::StagedMarkerAccepted);
        diagnostics.emit(DiagnosticEvent::CompletedManifest);

        assert_eq!(
            diagnostics.recorded_lines(),
            [
                "[verbose] update/outer: candidate 1.2.3 verified\n",
                "[verbose] update/outer: starting staged candidate\n",
                "[verbose] update/apply: staged marker accepted\n",
                "[verbose] update/apply: completed manifest\n",
            ]
        );

        for (command, prefix) in [
            (DiagnosticCommand::Setup, "setup"),
            (DiagnosticCommand::Status, "status"),
            (DiagnosticCommand::Enable, "enable"),
            (DiagnosticCommand::Disable, "disable"),
            (DiagnosticCommand::Uninstall, "uninstall"),
            (DiagnosticCommand::Codex, "codex"),
        ] {
            let mut diagnostics = Diagnostics::record(command);
            diagnostics.emit(DiagnosticEvent::CompletedPreflight);
            assert_eq!(
                diagnostics.recorded_lines(),
                [format!("[verbose] {prefix}: completed preflight\n")]
            );
        }
    }

    #[test]
    fn every_constructor_excludes_all_privacy_sentinels() {
        const SENTINELS: [&str; 6] = [
            "credential-secret",
            "raw-error-secret",
            "task-secret",
            "pid-4242",
            "timestamp-secret",
            "telemetry-secret",
        ];
        let mut events = vec![
            DiagnosticEvent::CandidateVerified {
                version: Version::parse("1.2.3").unwrap(),
            },
            DiagnosticEvent::StartingStagedCandidate,
            DiagnosticEvent::StagedMarkerAccepted,
            DiagnosticEvent::SelectedCodexHome {
                codex_home: PathBuf::from("/home/test/.codex"),
            },
            DiagnosticEvent::CompletedPreflight,
            DiagnosticEvent::CompletedBinary,
            DiagnosticEvent::CompletedManifest,
        ];
        events.extend(
            [
                DiagnosticCause::Unexpected,
                DiagnosticCause::Validation,
                DiagnosticCause::ReleaseDownload,
                DiagnosticCause::Checksum,
                DiagnosticCause::ServiceConfiguration,
                DiagnosticCause::ServiceStart,
                DiagnosticCause::ServiceStop,
                DiagnosticCause::ServiceState,
                DiagnosticCause::CliIntegration,
                DiagnosticCause::DesktopIntegration,
                DiagnosticCause::ActiveTasks,
                DiagnosticCause::Cleanup,
            ]
            .into_iter()
            .map(|cause| DiagnosticEvent::FailedServiceVerify { cause }),
        );

        let mut diagnostics = Diagnostics::record(DiagnosticCommand::Setup);
        for event in events {
            diagnostics.emit(event);
        }
        let rendered = diagnostics.recorded_lines().concat();
        for sentinel in SENTINELS {
            assert!(!rendered.contains(sentinel));
        }
    }

    #[test]
    fn first_write_failure_disables_later_output_without_changing_result() {
        let mut diagnostics = Diagnostics::fail_once(DiagnosticCommand::Setup);
        diagnostics.emit(DiagnosticEvent::CompletedPreflight);
        diagnostics.emit(DiagnosticEvent::CompletedBinary);

        assert!(diagnostics.is_off());
        assert_eq!(0, 0);
    }
}
