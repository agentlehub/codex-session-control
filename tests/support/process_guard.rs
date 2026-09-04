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
const PIPE_READ_ATTEMPTS_PER_TURN: usize = 4;
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
        if let Err(error) = set_nonblocking(&stdout) {
            return self.cleanup_after_error(io::Error::new(
                error.kind(),
                format!("child stdout nonblocking setup failed: {error}"),
            ));
        }
        if let Err(error) = set_nonblocking(&stderr) {
            return self.cleanup_after_error(io::Error::new(
                error.kind(),
                format!("child stderr nonblocking setup failed: {error}"),
            ));
        }

        let deadline = Instant::now() + timeout;
        let mut stdout_capture = PipeCapture::new();
        let mut stderr_capture = PipeCapture::new();
        let mut stdout_closed = false;
        let mut stderr_closed = false;
        let mut status = None;
        let mut stdout_first = true;

        loop {
            if Instant::now() >= deadline {
                return self.cleanup_after_error(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "child did not complete within {timeout:?}; stdout: {}; stderr: {}",
                        stdout_capture.summary(),
                        stderr_capture.summary()
                    ),
                ));
            }

            let stdout_before = stdout_capture.total_bytes;
            let stderr_before = stderr_capture.total_bytes;
            let drain_result = {
                let mut drain_stdout = || {
                    drain_open_pipe(
                        "stdout",
                        &mut stdout,
                        &mut stdout_capture,
                        &mut stdout_closed,
                        deadline,
                    )
                };
                let mut drain_stderr = || {
                    drain_open_pipe(
                        "stderr",
                        &mut stderr,
                        &mut stderr_capture,
                        &mut stderr_closed,
                        deadline,
                    )
                };
                if stdout_first {
                    drain_stdout().and_then(|limit_exceeded| {
                        if limit_exceeded {
                            Ok(true)
                        } else {
                            drain_stderr()
                        }
                    })
                } else {
                    drain_stderr().and_then(|limit_exceeded| {
                        if limit_exceeded {
                            Ok(true)
                        } else {
                            drain_stdout()
                        }
                    })
                }
            };
            stdout_first = !stdout_first;

            match drain_result {
                Ok(true) => {
                    return self.cleanup_after_error(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "child output exceeded capture limit; stdout: {}; stderr: {}",
                            stdout_capture.summary(),
                            stderr_capture.summary()
                        ),
                    ));
                }
                Ok(false) => {}
                Err(error) => return self.cleanup_after_error(error),
            }

            if status.is_none() {
                match self
                    .child
                    .as_mut()
                    .expect("child already reaped")
                    .try_wait()
                {
                    Ok(Some(exited)) => {
                        self.child.take();
                        if let Err(error) = self.terminate_process_group() {
                            return self.cleanup_after_error(io::Error::other(format!(
                                "process-group cleanup after child exit failed: {error}"
                            )));
                        }
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
                return Ok(Output {
                    status,
                    stdout: stdout_capture.bytes,
                    stderr: stderr_capture.bytes,
                });
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return self.cleanup_after_error(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "child did not complete within {timeout:?}; stdout: {}; stderr: {}",
                        stdout_capture.summary(),
                        stderr_capture.summary()
                    ),
                ));
            }

            if stdout_capture.total_bytes == stdout_before
                && stderr_capture.total_bytes == stderr_before
            {
                thread::sleep(CHILD_POLL_INTERVAL.min(remaining));
            }
        }
    }

    fn cleanup_after_error<T>(&mut self, error: io::Error) -> io::Result<T> {
        match self.terminate_and_reap() {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(io::Error::other(format!(
                "child operation failed: {error}; cleanup failed: {cleanup_error}"
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
    truncated: bool,
}

impl PipeCapture {
    pub fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(PIPE_CAPTURE_LIMIT),
            total_bytes: 0,
            truncated: false,
        }
    }

    fn append(&mut self, bytes: &[u8]) -> bool {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        let retained = bytes.len().min(PIPE_CAPTURE_LIMIT - self.bytes.len());
        self.bytes.extend_from_slice(&bytes[..retained]);
        let limit_exceeded = retained != bytes.len();
        self.truncated |= limit_exceeded;
        limit_exceeded
    }

    fn summary(&self) -> String {
        if self.truncated {
            format!("{} bytes (truncated)", self.total_bytes)
        } else {
            format!("{} bytes", self.total_bytes)
        }
    }
}

fn set_nonblocking(pipe: impl AsFd) -> io::Result<()> {
    let flags = fcntl_getfl(&pipe).map_err(io::Error::from)?;
    fcntl_setfl(&pipe, flags | OFlags::NONBLOCK).map_err(io::Error::from)
}

#[derive(Debug, PartialEq, Eq)]
pub enum DrainState {
    Open,
    Closed,
    LimitExceeded,
}

pub fn drain_pipe(
    mut pipe: impl Read,
    capture: &mut PipeCapture,
    deadline: Instant,
) -> io::Result<DrainState> {
    let mut buffer = [0u8; 8 * 1024];
    for _ in 0..PIPE_READ_ATTEMPTS_PER_TURN {
        if Instant::now() >= deadline {
            return Ok(DrainState::Open);
        }
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(DrainState::Closed),
            Ok(bytes_read) => {
                if capture.append(&buffer[..bytes_read]) {
                    return Ok(DrainState::LimitExceeded);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                return Ok(DrainState::Open);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(DrainState::Open)
}

fn drain_open_pipe(
    stream: &str,
    pipe: &mut impl Read,
    capture: &mut PipeCapture,
    closed: &mut bool,
    deadline: Instant,
) -> io::Result<bool> {
    if *closed {
        return Ok(false);
    }
    let state = drain_pipe(pipe, capture, deadline).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("child {stream} capture failed: {error}"),
        )
    })?;
    match state {
        DrainState::Open => Ok(false),
        DrainState::Closed => {
            *closed = true;
            Ok(false)
        }
        DrainState::LimitExceeded => Ok(true),
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
