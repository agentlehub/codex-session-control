#![allow(dead_code)] // Integration crates use distinct subsets of this shared guard.

use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use std::{
    io::{self, Read},
    os::fd::AsFd,
    os::unix::process::CommandExt,
    process::{Child, ChildStdin, Command, ExitStatus, Output},
    thread,
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
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("child stdout was not piped"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("child stderr was not piped"))?;
        set_nonblocking(&stdout)?;
        set_nonblocking(&stderr)?;

        let deadline = Instant::now() + timeout;
        let mut stdout_capture = PipeCapture::new();
        let mut stderr_capture = PipeCapture::new();
        let mut stdout_closed = false;
        let mut stderr_closed = false;
        let mut status = None;

        loop {
            stdout_closed = stdout_closed || drain_pipe(&mut stdout, &mut stdout_capture)?;
            stderr_closed = stderr_closed || drain_pipe(&mut stderr, &mut stderr_capture)?;

            if status.is_none() {
                match self
                    .child
                    .as_mut()
                    .expect("child already reaped")
                    .try_wait()
                {
                    Ok(Some(exited)) => {
                        self.child.take();
                        self.terminate_process_group()?;
                        status = Some(exited);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return self.cleanup_after_error(error);
                    }
                }
            }

            if let Some(status) = status
                && stdout_closed
                && stderr_closed
            {
                if stdout_capture.truncated || stderr_capture.truncated {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "child output exceeded capture limit; stdout: {}; stderr: {}",
                            stdout_capture.summary(),
                            stderr_capture.summary()
                        ),
                    ));
                }
                return Ok(Output {
                    status,
                    stdout: stdout_capture.bytes,
                    stderr: stderr_capture.bytes,
                });
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                if let Err(error) = self.terminate_and_reap() {
                    return Err(io::Error::other(format!(
                        "child cleanup failed after deadline: {error}"
                    )));
                }
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "child did not complete within {timeout:?}; stdout: {}; stderr: {}",
                        stdout_capture.summary(),
                        stderr_capture.summary()
                    ),
                ));
            }

            thread::sleep(CHILD_POLL_INTERVAL.min(remaining));
        }
    }

    fn cleanup_after_error<T>(&mut self, error: io::Error) -> io::Result<T> {
        match self.terminate_and_reap() {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(io::Error::other(format!(
                "child wait failed: {error}; cleanup failed: {cleanup_error}"
            ))),
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
    pub truncated: bool,
}

impl PipeCapture {
    pub fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(PIPE_CAPTURE_LIMIT),
            total_bytes: 0,
            truncated: false,
        }
    }

    fn append(&mut self, bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        let retained = bytes.len().min(PIPE_CAPTURE_LIMIT - self.bytes.len());
        self.bytes.extend_from_slice(&bytes[..retained]);
        self.truncated |= retained != bytes.len();
    }

    fn summary(&self) -> String {
        if self.truncated {
            format!("{} bytes (truncated)", self.total_bytes)
        } else {
            format!("{} bytes", self.total_bytes)
        }
    }
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

fn set_nonblocking(pipe: impl AsFd) -> io::Result<()> {
    let flags = fcntl_getfl(&pipe).map_err(io::Error::from)?;
    fcntl_setfl(&pipe, flags | OFlags::NONBLOCK).map_err(io::Error::from)
}

pub fn drain_pipe(mut pipe: impl Read, capture: &mut PipeCapture) -> io::Result<bool> {
    let mut buffer = [0u8; 8 * 1024];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(bytes_read) => capture.append(&buffer[..bytes_read]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(error),
        }
    }
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
