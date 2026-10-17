//! Bounded host control commands. Workload/session launches must use their own lifecycle bounds.
use std::{
    io::{self, Read},
    process::{Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("runtime control command timed out")]
    Timeout,
    #[error("runtime control command unavailable: {0}")]
    Io(#[from] io::Error),
    #[error("runtime control output unreadable: {0}")]
    Output(io::Error),
}

/// A captured pipe participates in the deadline even when its retained byte limit is zero.
/// Null and inherited streams create no pipe and therefore no EOF dependency.
#[derive(Clone, Copy)]
pub enum StreamPolicy {
    Capture(Option<usize>),
    Null,
    Inherit,
}

impl Default for StreamPolicy {
    fn default() -> Self {
        Self::Capture(None)
    }
}

impl StreamPolicy {
    fn stdio(self) -> Stdio {
        match self {
            Self::Capture(_) => Stdio::piped(),
            Self::Null => Stdio::null(),
            Self::Inherit => Stdio::inherit(),
        }
    }

    fn limit(self) -> Option<usize> {
        match self {
            Self::Capture(limit) => limit,
            Self::Null | Self::Inherit => None,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct OutputPolicy {
    pub stdout: StreamPolicy,
    pub stderr: StreamPolicy,
}

impl OutputPolicy {
    pub const STDOUT: Self = Self {
        stdout: StreamPolicy::Capture(None),
        stderr: StreamPolicy::Null,
    };
    pub const DISCARD: Self = Self {
        stdout: StreamPolicy::Null,
        stderr: StreamPolicy::Null,
    };
}

/// Like `Command::output`, capture both streams. Use an explicit policy for null/inherited streams;
/// command-builder stdio settings are replaced by that policy at the shared spawn boundary.
pub fn output(command: &mut Command) -> Result<Output, ControlError> {
    output_with_policy(command, OutputPolicy::default())
}

pub fn output_with_policy(
    command: &mut Command,
    policy: OutputPolicy,
) -> Result<Output, ControlError> {
    output_until(command, Instant::now() + CONTROL_TIMEOUT, policy)
}

/// The deadline includes spawn, pipe EOF and exit. It is checked before spawning. Callers with an
/// existing tighter budget pass that absolute deadline; no new budget is granted while offloading.
pub fn output_until(
    command: &mut Command,
    deadline: Instant,
    policy: OutputPolicy,
) -> Result<Output, ControlError> {
    execute_until(command, deadline, policy, None)
}

fn execute_until(
    command: &mut Command,
    deadline: Instant,
    policy: OutputPolicy,
    cancelled: Option<&AtomicBool>,
) -> Result<Output, ControlError> {
    let expired =
        || Instant::now() >= deadline || cancelled.is_some_and(|flag| flag.load(Ordering::Acquire));
    if expired() {
        return Err(ControlError::Timeout);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .stdin(Stdio::null())
        .stdout(policy.stdout.stdio())
        .stderr(policy.stderr.stdio());
    let mut child = command.spawn()?;
    let (tx, rx) = std::sync::mpsc::channel();
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let mut stdout = stdout_pipe.is_none().then(|| Ok(Vec::new()));
    let mut stderr = stderr_pipe.is_none().then(|| Ok(Vec::new()));
    for (is_stdout, stream, limit) in [
        (
            true,
            stdout_pipe.map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
            policy.stdout.limit(),
        ),
        (
            false,
            stderr_pipe.map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
            policy.stderr.limit(),
        ),
    ] {
        let Some(mut stream) = stream else {
            continue;
        };
        let tx = tx.clone();
        std::thread::spawn(move || {
            let result: io::Result<Vec<u8>> = (|| {
                let mut kept = Vec::new();
                let mut buf = [0; 8192];
                loop {
                    let n = match stream.read(&mut buf) {
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        result => result?,
                    };
                    if n == 0 {
                        return Ok(kept);
                    }
                    let take = limit.map_or(n, |cap| n.min(cap.saturating_sub(kept.len())));
                    kept.extend_from_slice(&buf[..take]);
                }
            })();
            let _ = tx.send((is_stdout, result));
        });
    }
    drop(tx);
    let result = loop {
        if expired() {
            break Err(ControlError::Timeout);
        }
        while let Ok((is_stdout, bytes)) = rx.try_recv() {
            if is_stdout {
                stdout = Some(bytes);
            } else {
                stderr = Some(bytes);
            }
        }
        if stdout.as_ref().is_some_and(Result::is_err) {
            break Err(ControlError::Output(stdout.take().unwrap().unwrap_err()));
        }
        if stderr.as_ref().is_some_and(Result::is_err) {
            break Err(ControlError::Output(stderr.take().unwrap().unwrap_err()));
        }
        // Keep the child unreaped until the pipes close. Its PID then remains reserved even when a
        // wrapper exits before a descendant, so group termination cannot target a reused PID.
        if stdout.is_some() && stderr.is_some() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    break Ok(Output {
                        status,
                        stdout: stdout.take().unwrap()?,
                        stderr: stderr.take().unwrap()?,
                    })
                }
                Ok(None) => {}
                Err(error) => break Err(error.into()),
            }
        }
        std::thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(5)),
        );
    };
    if result.is_err() {
        #[cfg(unix)]
        if let Ok(pid) = libc::pid_t::try_from(child.id()) {
            // SAFETY: this unreaped child owns the group established above.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

async fn execute_async(
    mut command: Command,
    deadline: Instant,
    policy: OutputPolicy,
) -> Result<Output, ControlError> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = CancelOnDrop(cancelled.clone());
    // Dropping the async waiter requests settlement; the blocking owner remains responsible for
    // killing the group and reaping the child, including when the async runtime is shutting down.
    tokio::task::spawn_blocking(move || {
        execute_until(&mut command, deadline, policy, Some(&cancelled))
    })
    .await
    .map_err(|error| ControlError::Io(io::Error::other(error)))?
}

/// Async callers offload the entire synchronous owner, including timeout settlement and reap.
pub async fn output_async(
    command: Command,
    deadline: Instant,
    policy: OutputPolicy,
) -> Result<Output, ControlError> {
    execute_async(command, deadline, policy).await
}

/// Preserve inherited stdout/stderr for control paths that previously waited on a spawned child.
/// The same deadline and process-group owner still settles failures before returning.
pub async fn status_async(
    command: Command,
    deadline: Instant,
) -> Result<std::process::ExitStatus, ControlError> {
    execute_async(
        command,
        deadline,
        OutputPolicy {
            stdout: StreamPolicy::Inherit,
            stderr: StreamPolicy::Inherit,
        },
    )
    .await
    .map(|out| out.status)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn shell(script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", script]);
        cmd
    }

    fn assert_dead(pid: i32) {
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            // A zombie has terminated; some container init processes defer orphan reaping.
            let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                stat.split_once(") ")
                    .is_some_and(|(_, rest)| rest.starts_with('Z'))
            });
            if zombie || unsafe { libc::kill(pid, 0) } != 0 {
                return;
            }
            assert!(Instant::now() < until, "process {pid} survived");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn deadline_kills_and_reaps_child_and_group() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("pid");
        let mut cmd = shell("echo $$ > \"$MARKER\"; exec sleep 60");
        cmd.env("MARKER", &marker);
        let started = Instant::now();
        assert!(matches!(
            output_until(
                &mut cmd,
                started + Duration::from_millis(200),
                Default::default()
            ),
            Err(ControlError::Timeout)
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
        let pid: i32 = std::fs::read_to_string(marker)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_dead(pid);
        assert_ne!(unsafe { libc::kill(-pid, 0) }, 0, "group survived");
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1,
            "child was not reaped"
        );
    }

    #[test]
    fn descendant_retaining_pipes_cannot_extend_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("child");
        let mut cmd = shell("sleep 60 & echo $! > \"$MARKER\"; exit 0");
        cmd.env("MARKER", &marker);
        let start = Instant::now();
        assert!(matches!(
            output_until(
                &mut cmd,
                start + Duration::from_millis(200),
                Default::default()
            ),
            Err(ControlError::Timeout)
        ));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_dead(
            std::fs::read_to_string(marker)
                .unwrap()
                .trim()
                .parse()
                .unwrap(),
        );
    }

    #[test]
    fn expired_deadline_never_spawns() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("spawned");
        let mut cmd = shell("touch \"$MARKER\"");
        cmd.env("MARKER", &marker);
        assert!(matches!(
            output_until(&mut cmd, Instant::now(), Default::default()),
            Err(ControlError::Timeout)
        ));
        assert!(!marker.exists());
    }

    #[test]
    fn fast_nonzero_is_not_timeout_and_output_is_complete() {
        let mut cmd = shell("printf 'stdout'; printf 'stderr' >&2; exit 7");
        let out = output(&mut cmd).unwrap();
        assert_eq!(out.status.code(), Some(7));
        assert_eq!(out.stdout, b"stdout");
        assert_eq!(out.stderr, b"stderr");
        let out = output(&mut shell("head -c 131072 /dev/zero; printf 'ok' >&2")).unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, vec![0; 131072]);
        assert_eq!(out.stderr, b"ok");
        let out = output_until(
            &mut shell("printf abcdef; printf secret >&2"),
            Instant::now() + Duration::from_secs(2),
            OutputPolicy {
                stdout: StreamPolicy::Capture(Some(3)),
                stderr: StreamPolicy::Capture(Some(0)),
            },
        )
        .unwrap();
        assert_eq!(out.stdout, b"abc");
        assert!(out.stderr.is_empty());
    }

    #[test]
    fn uncaptured_streams_do_not_wait_for_descendant_eof() {
        for policy in [
            OutputPolicy::STDOUT,
            OutputPolicy::DISCARD,
            OutputPolicy {
                stdout: StreamPolicy::Inherit,
                stderr: StreamPolicy::Inherit,
            },
        ] {
            for code in [0, 7] {
                let dir = tempfile::tempdir().unwrap();
                let marker = dir.path().join("child");
                let mut command = shell(
                    "sleep 60 >&2 & echo $! > \"$MARKER\"; printf 'exact-id'; exit \"$EXIT_CODE\"",
                );
                command
                    .env("MARKER", &marker)
                    .env("EXIT_CODE", code.to_string());
                let started = Instant::now();
                let result =
                    output_until(&mut command, started + Duration::from_millis(500), policy);
                let pid: i32 = std::fs::read_to_string(marker)
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                let independent_child_alive = unsafe { libc::kill(pid, 0) } == 0;
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
                assert_dead(pid);
                let out = result.unwrap();
                assert_eq!(out.status.code(), Some(code));
                assert!(independent_child_alive);
                if matches!(policy.stdout, StreamPolicy::Capture(_)) {
                    assert_eq!(out.stdout, b"exact-id");
                } else {
                    assert!(out.stdout.is_empty());
                }
                assert!(out.stderr.is_empty());
                assert!(started.elapsed() < Duration::from_secs(2));
            }
        }
    }

    #[test]
    fn zero_retained_stderr_still_bounds_the_captured_pipe() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("child");
        let mut command = shell("sleep 60 >&2 & echo $! > \"$MARKER\"; exit 0");
        command.env("MARKER", &marker);
        assert!(matches!(
            output_until(
                &mut command,
                Instant::now() + Duration::from_millis(200),
                OutputPolicy {
                    stdout: StreamPolicy::Capture(None),
                    stderr: StreamPolicy::Capture(Some(0)),
                }
            ),
            Err(ControlError::Timeout)
        ));
        assert_dead(
            std::fs::read_to_string(marker)
                .unwrap()
                .trim()
                .parse()
                .unwrap(),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_waiter_still_kills_and_reaps_owned_group() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("started");
        let mut cmd = shell("echo $$ > \"$MARKER\"; exec sleep 60");
        cmd.env("MARKER", &marker);
        let waiter = tokio::spawn(output_async(
            cmd,
            Instant::now() + CONTROL_TIMEOUT,
            Default::default(),
        ));
        let until = Instant::now() + Duration::from_secs(2);
        let pid: i32 = loop {
            if let Some(pid) = std::fs::read_to_string(&marker)
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                break pid;
            }
            assert!(Instant::now() < until, "runtime did not record its PID");
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert_dead(pid);
        let settled_by = Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(-pid, 0) } == 0 {
            assert!(
                Instant::now() < settled_by,
                "cancelled group was not reaped"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_wait_keeps_current_thread_runtime_responsive() {
        let ticks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call = async {
            let result = output_async(
                shell("exec sleep 60"),
                Instant::now() + Duration::from_secs(1),
                Default::default(),
            )
            .await;
            (result, ticks.load(Ordering::Acquire))
        };
        let ticker = async {
            for _ in 0..10 {
                tokio::time::sleep(Duration::from_millis(10)).await;
                ticks.fetch_add(1, Ordering::Release);
            }
        };
        let ((result, ticks_before_settlement), ()) = tokio::join!(call, ticker);
        assert!(matches!(result, Err(ControlError::Timeout)));
        assert!(
            ticks_before_settlement > 0,
            "ticker was blocked until the command settled"
        );
    }
}
