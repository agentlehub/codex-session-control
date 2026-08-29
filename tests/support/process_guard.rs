#![allow(dead_code)] // Integration crates use distinct subsets of this shared guard.

use std::{
    io::{self, Read},
    os::unix::process::CommandExt,
    process::{Child, ChildStdin, Command, ExitStatus, Output},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);
pub const PIPE_CAPTURE_LIMIT: usize = 64 * 1024;

pub struct ChildGuard {
    child: Option<Child>,
    process_group: Option<u32>,
}

impl ChildGuard {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        command.process_group(0);
        command.spawn().map(|child| Self {
            process_group: Some(child.id()),
            child: Some(child),
        })
    }

    pub fn id(&self) -> u32 {
        self.child.as_ref().expect("child already reaped").id()
    }

    pub fn stdin_mut(&mut self) -> Option<&mut ChildStdin> {
        self.child.as_mut()?.stdin.as_mut()
    }

    pub fn close_stdin(&mut self) {
        drop(
            self.child
                .as_mut()
                .expect("child already reaped")
                .stdin
                .take(),
        );
    }

    pub fn wait_with_output(mut self, timeout: Duration) -> io::Result<Output> {
        let child = self.child.as_mut().expect("child already reaped");
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("child stdout was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("child stderr was not piped"))?;
        let stdout_reader = read_pipe(stdout);
        let stderr_reader = read_pipe(stderr);
        let deadline = Instant::now() + timeout;

        loop {
            match self
                .child
                .as_mut()
                .expect("child already reaped")
                .try_wait()
            {
                Ok(Some(status)) => {
                    self.child.take();
                    let cleanup_result = self.terminate_process_group();
                    let output_result = collect_output(status, stdout_reader, stderr_reader);
                    cleanup_result?;
                    return output_result;
                }
                Ok(None) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        self.terminate_and_reap()?;
                        let stdout_result = join_pipe(stdout_reader, "stdout");
                        let stderr_result = join_pipe(stderr_reader, "stderr");
                        let stdout = stdout_result?;
                        let stderr = stderr_result?;
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!(
                                "child did not exit within {timeout:?}; stdout: {}; stderr: {}",
                                stdout, stderr
                            ),
                        ));
                    }
                    thread::sleep(CHILD_POLL_INTERVAL.min(remaining));
                }
                Err(error) => {
                    self.terminate_and_reap()?;
                    let stdout_result = join_pipe(stdout_reader, "stdout");
                    let stderr_result = join_pipe(stderr_reader, "stderr");
                    let _ = stdout_result?;
                    let _ = stderr_result?;
                    return Err(error);
                }
            }
        }
    }

    fn terminate_and_reap(&mut self) -> io::Result<()> {
        let group_result = self.terminate_process_group();
        let child_result = self.terminate_child();
        match (group_result, child_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(group_error), Err(child_error)) => Err(io::Error::other(format!(
                "process-group cleanup failed: {group_error}; child cleanup failed: {child_error}"
            ))),
        }
    }

    fn terminate_process_group(&mut self) -> io::Result<()> {
        let Some(process_group) = self.process_group else {
            return Ok(());
        };
        let process_group = rustix::process::Pid::from_raw(process_group as i32)
            .ok_or_else(|| io::Error::other("process group id is zero"))?;
        match rustix::process::kill_process_group(process_group, rustix::process::Signal::KILL) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => {
                self.process_group.take();
                Ok(())
            }
            Err(error) => Err(io::Error::from(error)),
        }
    }

    fn terminate_child(&mut self) -> io::Result<()> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        if let Ok(Some(_)) = child.try_wait() {
            self.child.take();
            return Ok(());
        }
        if let Err(kill_error) = child.kill() {
            return match child.try_wait() {
                Ok(Some(_)) => {
                    self.child.take();
                    Ok(())
                }
                Ok(None) => Err(kill_error),
                Err(recheck_error) => Err(io::Error::other(format!(
                    "child kill failed: {kill_error}; exit recheck failed: {recheck_error}"
                ))),
            };
        }
        match child.wait() {
            Ok(_) => {
                self.child.take();
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.terminate_and_reap();
    }
}

pub struct PipeCapture {
    pub bytes: Vec<u8>,
    pub total_bytes: usize,
}

fn write_lossy_bytes(
    formatter: &mut std::fmt::Formatter<'_>,
    mut bytes: &[u8],
) -> std::fmt::Result {
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(text) => return formatter.write_str(text),
            Err(error) => {
                let valid_len = error.valid_up_to();
                if valid_len > 0 {
                    let valid = std::str::from_utf8(&bytes[..valid_len])
                        .expect("valid_up_to must delimit valid UTF-8");
                    formatter.write_str(valid)?;
                }
                formatter.write_str("?")?;
                let invalid_len = error.error_len().unwrap_or(bytes.len() - valid_len);
                bytes = &bytes[valid_len + invalid_len..];
            }
        }
    }
    Ok(())
}

impl std::fmt::Display for PipeCapture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write_lossy_bytes(formatter, &self.bytes)?;
        if self.total_bytes > self.bytes.len() {
            write!(
                formatter,
                " [truncated: captured first {} of {} bytes]",
                self.bytes.len(),
                self.total_bytes
            )?;
        }
        Ok(())
    }
}

pub fn read_pipe(mut pipe: impl Read + Send + 'static) -> JoinHandle<io::Result<PipeCapture>> {
    thread::spawn(move || {
        let mut captured = Vec::with_capacity(PIPE_CAPTURE_LIMIT);
        let mut total_bytes = 0usize;
        let mut buffer = [0u8; 8 * 1024];
        loop {
            let bytes_read = match pipe.read(&mut buffer) {
                Ok(bytes_read) => bytes_read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if bytes_read == 0 {
                break;
            }
            total_bytes = total_bytes.saturating_add(bytes_read);
            let retained = bytes_read.min(PIPE_CAPTURE_LIMIT - captured.len());
            captured.extend_from_slice(&buffer[..retained]);
        }
        Ok(PipeCapture {
            bytes: captured,
            total_bytes,
        })
    })
}

pub fn join_pipe(
    reader: JoinHandle<io::Result<PipeCapture>>,
    stream: &str,
) -> io::Result<PipeCapture> {
    reader
        .join()
        .map_err(|_| io::Error::other(format!("{stream} reader panicked")))?
}

fn collect_output(
    status: ExitStatus,
    stdout_reader: JoinHandle<io::Result<PipeCapture>>,
    stderr_reader: JoinHandle<io::Result<PipeCapture>>,
) -> io::Result<Output> {
    let stdout = join_pipe(stdout_reader, "stdout");
    let stderr = join_pipe(stderr_reader, "stderr");
    Ok(Output {
        status,
        stdout: stdout?.bytes,
        stderr: stderr?.bytes,
    })
}

pub fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        thread::sleep(CHILD_POLL_INTERVAL.min(remaining));
    }
}

pub fn terminate_test_child(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    if child.kill().is_ok() {
        let _ = child.wait();
    }
}
