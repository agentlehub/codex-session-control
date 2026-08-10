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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiagnosticTarget {
    #[cfg(target_arch = "x86_64")]
    X86_64Linux,
    #[cfg(target_arch = "aarch64")]
    Aarch64Linux,
}

impl DiagnosticTarget {
    pub(crate) fn current() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            Self::X86_64Linux
        }
        #[cfg(target_arch = "aarch64")]
        {
            Self::Aarch64Linux
        }
    }

    fn label(self) -> &'static str {
        match self {
            #[cfg(target_arch = "x86_64")]
            Self::X86_64Linux => "x86_64-unknown-linux-gnu",
            #[cfg(target_arch = "aarch64")]
            Self::Aarch64Linux => "aarch64-unknown-linux-gnu",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DiagnosticEvent {
    ControllerStarted {
        version: Version,
        target: DiagnosticTarget,
    },
    CandidateVerified {
        version: Version,
    },
    StartingStagedCandidate,
    StagedCandidateExitedSuccessfully,
    StagedMarkerAccepted,
    CompletedServiceRestart,
    SelectedCodexHome {
        codex_home: PathBuf,
    },
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
    pub(crate) fn record(command: DiagnosticCommand) -> Self {
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
        let detail = match event {
            DiagnosticEvent::ControllerStarted { version, target } => {
                format!("controller {version} ({})", target.label())
            }
            DiagnosticEvent::CandidateVerified { version } => {
                format!("candidate {version} verified")
            }
            DiagnosticEvent::StartingStagedCandidate => "starting staged candidate".to_owned(),
            DiagnosticEvent::StagedCandidateExitedSuccessfully => {
                "staged candidate exited successfully".to_owned()
            }
            DiagnosticEvent::StagedMarkerAccepted => "staged marker accepted".to_owned(),
            DiagnosticEvent::CompletedServiceRestart => "completed service-restart".to_owned(),
            DiagnosticEvent::SelectedCodexHome { codex_home } => {
                format!("selected Codex home {}", codex_home.display())
            }
        };
        self.write_detail(&detail);
    }

    pub(crate) fn completed(&mut self, stage: &'static str) {
        self.write_detail(&format!("completed {stage}"));
    }

    pub(crate) fn failed(&mut self, stage: &'static str, cause: DiagnosticCause) {
        self.write_detail(&format!("failed {stage} ({})", cause.label()));
    }

    fn write_detail(&mut self, detail: &str) {
        let line = self.render_line(detail);
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

    fn render_line(&self, detail: &str) -> String {
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
        format!("[verbose] {command}{phase}: {detail}\n")
    }

    #[cfg(test)]
    pub(crate) fn recorded_lines(&self) -> &[String] {
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
            Self::Unexpected => "unexpected failure",
            Self::Validation => "validation failed",
            Self::ReleaseDownload => "release could not be retrieved",
            Self::Checksum => "downloaded release could not be verified",
            Self::ServiceConfiguration => "service could not be configured",
            Self::ServiceStart => "service could not be started",
            Self::ServiceStop => "service could not be stopped",
            Self::ServiceState => "service state could not be verified",
            Self::CliIntegration => "Codex CLI integration could not be updated",
            Self::DesktopIntegration => "Codex Desktop integration could not be updated",
            Self::ActiveTasks => "active tasks could not be checked safely",
            Self::Cleanup => "cleanup could not be completed safely",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use semver::Version;

    use super::*;

    #[derive(Debug, Eq, PartialEq)]
    struct OperationEvidence {
        result: Result<&'static str, &'static str>,
        mutations: Vec<&'static str>,
        exit_code: u8,
    }

    fn execute_with_diagnostics(mut diagnostics: Diagnostics) -> (OperationEvidence, Diagnostics) {
        diagnostics.completed("preflight");
        let evidence = OperationEvidence {
            result: Err("original-result"),
            mutations: vec!["original-mutation"],
            exit_code: 1,
        };
        diagnostics.completed("binary");
        diagnostics.flush();
        (evidence, diagnostics)
    }

    #[test]
    fn prefixes_and_update_phases_are_exact() {
        let mut diagnostics = Diagnostics::record(DiagnosticCommand::Update);
        diagnostics.set_phase(UpdatePhase::Outer);
        diagnostics.emit(DiagnosticEvent::CandidateVerified {
            version: Version::parse("1.2.3").unwrap(),
        });
        diagnostics.emit(DiagnosticEvent::StartingStagedCandidate);
        diagnostics.emit(DiagnosticEvent::StagedCandidateExitedSuccessfully);
        diagnostics.set_phase(UpdatePhase::Apply);
        diagnostics.emit(DiagnosticEvent::StagedMarkerAccepted);
        diagnostics.emit(DiagnosticEvent::CompletedServiceRestart);
        diagnostics.completed("manifest");
        diagnostics.failed("service-verify", DiagnosticCause::ServiceState);

        assert_eq!(
            diagnostics.recorded_lines(),
            [
                "[verbose] update/outer: candidate 1.2.3 verified\n",
                "[verbose] update/outer: starting staged candidate\n",
                "[verbose] update/outer: staged candidate exited successfully\n",
                "[verbose] update/apply: staged marker accepted\n",
                "[verbose] update/apply: completed service-restart\n",
                "[verbose] update/apply: completed manifest\n",
                "[verbose] update/apply: failed service-verify (service state could not be verified)\n",
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
            diagnostics.completed("preflight");
            assert_eq!(
                diagnostics.recorded_lines(),
                [format!("[verbose] {prefix}: completed preflight\n")]
            );
        }

        let mut diagnostics = Diagnostics::record(DiagnosticCommand::Setup);
        diagnostics.emit(DiagnosticEvent::SelectedCodexHome {
            codex_home: PathBuf::from("/home/test/.codex"),
        });
        assert_eq!(
            diagnostics.recorded_lines(),
            ["[verbose] setup: selected Codex home /home/test/.codex\n"]
        );
    }

    #[test]
    fn every_dynamic_diagnostic_field_is_exactly_rendered() {
        let mut diagnostics = Diagnostics::record(DiagnosticCommand::Setup);
        for event in [
            DiagnosticEvent::ControllerStarted {
                version: Version::parse("1.2.3").unwrap(),
                target: DiagnosticTarget::current(),
            },
            DiagnosticEvent::CandidateVerified {
                version: Version::parse("1.2.3").unwrap(),
            },
            DiagnosticEvent::StartingStagedCandidate,
            DiagnosticEvent::StagedCandidateExitedSuccessfully,
            DiagnosticEvent::StagedMarkerAccepted,
            DiagnosticEvent::CompletedServiceRestart,
            DiagnosticEvent::SelectedCodexHome {
                codex_home: PathBuf::from("/home/test/.codex"),
            },
        ] {
            diagnostics.emit(event);
        }
        for cause in [
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
        ] {
            diagnostics.failed("service-verify", cause);
        }

        assert_eq!(
            diagnostics.recorded_lines(),
            [
                format!(
                    "[verbose] setup: controller 1.2.3 ({})\n",
                    DiagnosticTarget::current().label()
                ),
                "[verbose] setup: candidate 1.2.3 verified\n".to_owned(),
                "[verbose] setup: starting staged candidate\n".to_owned(),
                "[verbose] setup: staged candidate exited successfully\n".to_owned(),
                "[verbose] setup: staged marker accepted\n".to_owned(),
                "[verbose] setup: completed service-restart\n".to_owned(),
                "[verbose] setup: selected Codex home /home/test/.codex\n".to_owned(),
                "[verbose] setup: failed service-verify (unexpected failure)\n".to_owned(),
                "[verbose] setup: failed service-verify (validation failed)\n".to_owned(),
                "[verbose] setup: failed service-verify (release could not be retrieved)\n"
                    .to_owned(),
                "[verbose] setup: failed service-verify (downloaded release could not be verified)\n"
                    .to_owned(),
                "[verbose] setup: failed service-verify (service could not be configured)\n"
                    .to_owned(),
                "[verbose] setup: failed service-verify (service could not be started)\n"
                    .to_owned(),
                "[verbose] setup: failed service-verify (service could not be stopped)\n"
                    .to_owned(),
                "[verbose] setup: failed service-verify (service state could not be verified)\n"
                    .to_owned(),
                "[verbose] setup: failed service-verify (Codex CLI integration could not be updated)\n"
                    .to_owned(),
                "[verbose] setup: failed service-verify (Codex Desktop integration could not be updated)\n"
                    .to_owned(),
                "[verbose] setup: failed service-verify (active tasks could not be checked safely)\n"
                    .to_owned(),
                "[verbose] setup: failed service-verify (cleanup could not be completed safely)\n"
                    .to_owned(),
            ]
        );
    }

    #[test]
    fn first_write_failure_disables_later_output_without_changing_result() {
        let (observed, diagnostics) =
            execute_with_diagnostics(Diagnostics::fail_once(DiagnosticCommand::Setup));

        assert!(diagnostics.is_off());
        assert_eq!(
            observed,
            OperationEvidence {
                result: Err("original-result"),
                mutations: vec!["original-mutation"],
                exit_code: 1,
            }
        );
    }
}
