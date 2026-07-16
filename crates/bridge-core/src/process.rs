// supervisor.rs — subprocess supervisor with process-group SIGTERM/SIGKILL on terminate().
// Spec §9 + S3, Codex finding 6: SIGKILL targets the whole group so TERM-ignoring children
// and their descendants cannot survive.

use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStderr, Command};

use crate::diagnostics::DiagnosticRedactor;

const STDERR_RING_CAPACITY: usize = 32;
const STDERR_LINE_MAX_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessStderrCursor(u64);

#[derive(Clone)]
struct CapturedStderrLine {
    sequence: u64,
    #[allow(dead_code)]
    captured_at_ms: i64,
    text: String,
}

#[derive(Default)]
struct ProcessStderrState {
    sequence: u64,
    lines: VecDeque<CapturedStderrLine>,
    redactor: DiagnosticRedactor,
    /// Monotonic fail-closed mode used when a process may still emit a
    /// credential whose exact value can no longer be retained in a bounded
    /// redaction policy. Sequence/count metadata remains available, but text is
    /// replaced before retention and can never be re-enabled for this process.
    metadata_only: bool,
}

/// Bounded process-scoped stderr evidence. Lines are retained only in memory;
/// callers receive metadata by default and must not infer task ownership from a
/// process shared by concurrent attempts.
#[derive(Clone)]
pub struct ProcessStderrRing {
    state: Arc<Mutex<ProcessStderrState>>,
}

impl Default for ProcessStderrRing {
    fn default() -> Self {
        Self::new(DiagnosticRedactor::default())
    }
}

impl std::fmt::Debug for ProcessStderrRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.lock().ok();
        f.debug_struct("ProcessStderrRing")
            .field("capacity", &STDERR_RING_CAPACITY)
            .field(
                "line_count",
                &state.as_ref().map_or(0, |state| state.sequence),
            )
            .field(
                "metadata_only",
                &state.as_ref().is_some_and(|state| state.metadata_only),
            )
            .field("redactor", &state.as_ref().map(|state| &state.redactor))
            .finish()
    }
}

impl ProcessStderrRing {
    fn new(redactor: DiagnosticRedactor) -> Self {
        Self {
            state: Arc::new(Mutex::new(ProcessStderrState {
                redactor,
                ..ProcessStderrState::default()
            })),
        }
    }

    /// Replace the process redactor and retroactively sanitize the already
    /// bounded retained tail. The ring lock makes adoption race-safe with the
    /// drain task: lines captured before installation are rewritten, and later
    /// lines observe the new policy before entering memory.
    pub fn apply_redactor(&self, redactor: DiagnosticRedactor) {
        let mut state = self.state.lock().expect("process stderr ring lock");
        if state.metadata_only {
            for line in &mut state.lines {
                line.text = "[REDACTED LINE]".to_owned();
            }
        } else {
            for line in &mut state.lines {
                line.text = redactor.sanitize_stderr_line(&line.text, STDERR_LINE_MAX_BYTES);
            }
        }
        state.redactor = redactor;
    }

    /// Permanently disable retained stderr text for this process while keeping
    /// bounded line-count metadata. Existing lines are rewritten under the
    /// drain lock and future redactor replacement cannot re-enable text.
    pub fn retain_metadata_only(&self) {
        let mut state = self.state.lock().expect("process stderr ring lock");
        state.metadata_only = true;
        for line in &mut state.lines {
            line.text = "[REDACTED LINE]".to_owned();
        }
    }

    #[must_use]
    pub fn is_metadata_only(&self) -> bool {
        self.state.lock().is_ok_and(|state| state.metadata_only)
    }

    pub fn origin(&self) -> ProcessStderrCursor {
        ProcessStderrCursor(0)
    }

    pub fn cursor(&self) -> ProcessStderrCursor {
        ProcessStderrCursor(self.state.lock().map_or(0, |state| state.sequence))
    }

    fn push(&self, line: String, oversized: bool) {
        let mut state = self.state.lock().expect("process stderr ring lock");
        state.sequence = state.sequence.saturating_add(1);
        let sequence = state.sequence;
        let captured_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|duration| i64::try_from(duration.as_millis()).ok())
            .unwrap_or(0);
        // An oversized logical record is not partially retained: a credential
        // can straddle the storage boundary, where exact-value redaction could
        // otherwise leave a secret prefix in the ring. Metadata still records
        // that the line existed.
        let text = if oversized || state.metadata_only {
            "[REDACTED LINE]".to_owned()
        } else {
            state
                .redactor
                .sanitize_stderr_line(&line, STDERR_LINE_MAX_BYTES)
        };
        if state.lines.len() == STDERR_RING_CAPACITY {
            state.lines.pop_front();
        }
        state.lines.push_back(CapturedStderrLine {
            sequence,
            captured_at_ms,
            text,
        });
    }

    /// Snapshot bounded lines after `cursor` for an operation that explicitly opted into
    /// best-effort-redacted process text. Callers must keep the default diagnostic path on
    /// [`Self::metadata_since`]. The ring has already applied exact-value and shape redaction.
    pub fn best_effort_since(&self, cursor: ProcessStderrCursor) -> ProcessStderrSnapshot {
        self.snapshot_since_inner(cursor, true)
    }

    pub fn metadata_since(&self, cursor: ProcessStderrCursor) -> ProcessStderrSnapshot {
        self.snapshot_since_inner(cursor, false)
    }

    fn snapshot_since_inner(
        &self,
        cursor: ProcessStderrCursor,
        include_retained_text: bool,
    ) -> ProcessStderrSnapshot {
        let state = self.state.lock().expect("process stderr ring lock");
        let line_count = state.sequence.saturating_sub(cursor.0).min(u32::MAX as u64) as u32;
        let retained_lines = if include_retained_text {
            state
                .lines
                .iter()
                .filter(|line| line.sequence > cursor.0)
                .map(|line| line.text.clone())
                .collect()
        } else {
            Vec::new()
        };
        ProcessStderrSnapshot {
            line_count,
            retained_lines,
        }
    }
}

#[derive(Clone)]
pub struct ProcessStderrSnapshot {
    line_count: u32,
    retained_lines: Vec<String>,
}

impl std::fmt::Debug for ProcessStderrSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessStderrSnapshot")
            .field("line_count", &self.line_count)
            .field("scope", &crate::diagnostics::StderrScope::Process)
            .field("retained_line_count", &self.retained_lines.len())
            .finish()
    }
}

impl ProcessStderrSnapshot {
    pub fn line_count(&self) -> u32 {
        self.line_count
    }

    pub fn scope(&self) -> crate::diagnostics::StderrScope {
        crate::diagnostics::StderrScope::Process
    }

    /// Bounded, already-redacted lines. Metadata-only snapshots return an empty slice.
    pub fn retained_lines(&self) -> &[String] {
        &self.retained_lines
    }
}

pub struct Supervised {
    child: Child,
    pid: u32,
    stderr_ring: ProcessStderrRing,
}

impl Supervised {
    pub fn spawn(
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
    ) -> std::io::Result<Self> {
        Self::spawn_with_stderr_redactor(prog, args, cwd, DiagnosticRedactor::default())
    }

    /// Spawn a supervised child and sanitize process stderr with the supplied
    /// bridge-known credential set before any text enters the bounded ring.
    pub fn spawn_with_stderr_redactor(
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
        stderr_redactor: DiagnosticRedactor,
    ) -> std::io::Result<Self> {
        Self::spawn_with_stderr_redactor_and_pinned_cwd(
            prog,
            args,
            cwd,
            stderr_redactor,
            None,
            false,
        )
    }

    /// Spawn after changing the forked child to one already-open directory descriptor. The bridge
    /// parent's descriptor stays close-on-exec; the child may retain only its forked copy when its
    /// absolute object path requires the descriptor after exec.
    pub fn spawn_with_stderr_redactor_and_pinned_cwd(
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
        stderr_redactor: DiagnosticRedactor,
        pinned_cwd_fd: Option<std::os::fd::RawFd>,
        retain_pinned_cwd_fd_after_exec: bool,
    ) -> std::io::Result<Self> {
        let mut cmd = Command::new(prog);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0) // child becomes its own group leader (pgid == child pid)
            .kill_on_drop(true);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        if let Some(fd) = pinned_cwd_fd {
            // SAFETY: this callback runs in the forked child after fork and before exec. It performs
            // only async-signal-safe fchdir/fcntl calls. The parent descriptor remains FD_CLOEXEC.
            // When Linux needs /proc/self/fd/N as the ACP cwd, only this already-forked child clears
            // its copy, avoiding a concurrent-spawn inheritance window in the bridge process.
            unsafe {
                cmd.pre_exec(move || {
                    if libc::fchdir(fd) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if retain_pinned_cwd_fd_after_exec {
                        let flags = libc::fcntl(fd, libc::F_GETFD);
                        if flags == -1
                            || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1
                        {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                    Ok(())
                });
            }
        }
        let mut child = cmd.spawn()?;
        let pid = child.id().expect("child has a pid before wait");
        // Drain the child's stderr on a detached task. stderr is piped (so it never
        // interleaves with our own logs) but NOTHING else reads it: an agent that
        // writes past the ~64KB pipe buffer blocks on its next stderr write and
        // deadlocks its entire turn (observed live with a chatty ACP agent — the
        // turn hung, then the process died as AgentCrashed). Reading to EOF keeps
        // the pipe drained. Text remains only in the bounded in-memory ring;
        // opaque agent stderr never enters tracing.
        let stderr_ring = ProcessStderrRing::new(stderr_redactor);
        if let Some(stderr) = child.stderr.take() {
            let stderr_ring = stderr_ring.clone();
            tokio::spawn(drain_stderr(stderr, stderr_ring));
        }
        Ok(Self {
            child,
            pid,
            stderr_ring,
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn stderr_ring(&self) -> ProcessStderrRing {
        self.stderr_ring.clone()
    }

    /// Install a known-value redactor when adopting an already-spawned child.
    /// Existing retained lines are re-sanitized under the same lock used by the
    /// drain, so no capture/adoption race can leave literal known values behind.
    pub fn apply_stderr_redactor(&self, redactor: DiagnosticRedactor) {
        self.stderr_ring.apply_redactor(redactor);
    }

    /// Task 8 reads stdout/stdin via this.
    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// SIGTERM the group, wait `grace`, SIGKILL the group, reap.
    pub async fn terminate(mut self, grace: std::time::Duration) {
        let pgid = self.pid as i32;
        // SAFETY: kill(-pgid, sig) sends signal to a process group we own.
        // pgid is the child's pid, which equals the group leader pid because
        // we spawned with process_group(0).
        unsafe {
            libc::kill(-pgid, libc::SIGTERM);
        }
        if tokio::time::timeout(grace, self.child.wait())
            .await
            .is_err()
        {
            // Grace period elapsed — escalate to SIGKILL on the whole group.
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }
        // Always reap to prevent zombies.
        let _ = self.child.wait().await;
    }
}

/// Drain stderr without ever allocating in proportion to a peer-controlled
/// logical line. `AsyncBufReadExt::lines` is deliberately avoided: it buffers
/// until a newline and therefore lets a single unterminated record grow without
/// bound. We retain at most `STDERR_LINE_MAX_BYTES`; longer records become a
/// fixed redaction marker so a bridge-known credential cannot straddle the cap.
async fn drain_stderr(mut stderr: ChildStderr, ring: ProcessStderrRing) {
    let mut chunk = [0_u8; 4096];
    let mut retained = Vec::with_capacity(STDERR_LINE_MAX_BYTES);
    let mut line_started = false;
    let mut oversized = false;

    loop {
        let read = match stderr.read(&mut chunk).await {
            Ok(read) => read,
            Err(_) => return,
        };
        if read == 0 {
            if line_started {
                if !oversized && retained.last() == Some(&b'\r') {
                    retained.pop();
                }
                ring.push(String::from_utf8_lossy(&retained).into_owned(), oversized);
            }
            return;
        }

        for byte in &chunk[..read] {
            if *byte == b'\n' {
                if !oversized && retained.last() == Some(&b'\r') {
                    retained.pop();
                }
                ring.push(String::from_utf8_lossy(&retained).into_owned(), oversized);
                retained.clear();
                line_started = false;
                oversized = false;
                continue;
            }

            line_started = true;
            if retained.len() < STDERR_LINE_MAX_BYTES {
                retained.push(*byte);
            } else {
                oversized = true;
            }
        }
    }
}

impl Drop for Supervised {
    /// Backstop GROUP-kill on every drop path. A backend that is plain-DROPPED — normal
    /// `run-workflow`/`implement` completion, where the graceful async [`Supervised::terminate`] is NOT
    /// called — must not leak the agent's descendants. `kill_on_drop` reaps only the DIRECT child; the
    /// spawned process group (`pgid == self.pid` via `process_group(0)`) can hold descendants. e.g. the
    /// `codex-acp` node wrapper `spawnSync`s the real `codex` binary into its group; without a group kill
    /// here that `codex` orphans (reparents to init) and accumulates across runs. SIGKILL the whole group
    /// (best-effort: `ESRCH` after a prior `terminate()` already killed it is fine). `kill_on_drop` then
    /// reaps the leader, so the group leader never becomes a zombie.
    fn drop(&mut self) {
        // SAFETY: kill(-pgid, SIGKILL) targets a process group we own — `pgid == self.pid`, the group
        // leader, because we spawned with `process_group(0)`.
        unsafe {
            libc::kill(-(self.pid as i32), libc::SIGKILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd as _;
    use std::time::Duration;

    // returns the `stat`/`state` column for a pid, or None if the pid is gone/reaped.
    fn proc_state(pid: u32) -> Option<String> {
        let out = std::process::Command::new("ps")
            .args(["-o", "state=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    #[tokio::test]
    async fn pinned_directory_fd_changes_only_the_child_process_cwd() {
        use tokio::io::AsyncReadExt as _;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("original-marker"), b"ok").unwrap();
        let directory = std::fs::File::open(dir.path()).unwrap();
        let fd = directory.as_raw_fd();
        let parent_flags_before = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert_ne!(parent_flags_before & libc::FD_CLOEXEC, 0);

        let mut pinned = Supervised::spawn_with_stderr_redactor_and_pinned_cwd(
            "/bin/sh",
            &["-c", "test -f original-marker && echo pinned"],
            None,
            DiagnosticRedactor::default(),
            Some(fd),
            false,
        )
        .unwrap();
        let mut stdout = String::new();
        pinned
            .child_mut()
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut stdout)
            .await
            .unwrap();
        assert_eq!(stdout.trim(), "pinned");

        let parent_flags_after = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert_ne!(parent_flags_after & libc::FD_CLOEXEC, 0);

        let mut unpinned = Supervised::spawn_with_stderr_redactor_and_pinned_cwd(
            "/bin/sh",
            &["-c", "test ! -f original-marker && echo unpinned"],
            None,
            DiagnosticRedactor::default(),
            None,
            false,
        )
        .unwrap();
        let mut stdout = String::new();
        unpinned
            .child_mut()
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut stdout)
            .await
            .unwrap();
        assert_eq!(stdout.trim(), "unpinned");
    }

    #[tokio::test]
    async fn retained_pinned_directory_fd_is_visible_only_in_the_execed_child() {
        use tokio::io::AsyncReadExt as _;

        let dir = tempfile::tempdir().unwrap();
        let directory = std::fs::File::open(dir.path()).unwrap();
        let fd = directory.as_raw_fd();
        #[cfg(target_os = "macos")]
        let child_handle = format!("/dev/fd/{fd}");
        #[cfg(target_os = "linux")]
        let child_handle = format!("/proc/self/fd/{fd}");
        let command = format!("test -e {child_handle} && echo retained");
        let mut child = Supervised::spawn_with_stderr_redactor_and_pinned_cwd(
            "/bin/sh",
            &["-c", &command],
            None,
            DiagnosticRedactor::default(),
            Some(fd),
            true,
        )
        .unwrap();
        let mut stdout = String::new();
        child
            .child_mut()
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut stdout)
            .await
            .unwrap();

        assert_eq!(stdout.trim(), "retained");
        let parent_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert_ne!(parent_flags & libc::FD_CLOEXEC, 0);
    }

    #[tokio::test]
    async fn chatty_stderr_does_not_deadlock_the_turn() {
        // Regression: stderr was piped but never drained, so a child writing past
        // the ~64KB pipe buffer blocked on its next stderr write and never produced
        // stdout (a live ACP agent hung this way, then died as AgentCrashed). The
        // child below writes ~320KB to stderr, THEN a sentinel to stdout. If stderr
        // is undrained the stdout read hangs; with draining it returns "DONE".
        use tokio::io::AsyncReadExt;
        let mut sup = Supervised::spawn(
            "/bin/sh",
            &[
                "-c",
                "i=0; while [ $i -lt 8000 ]; do \
                 echo xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx 1>&2; \
                 i=$((i+1)); done; echo DONE",
            ],
            None,
        )
        .unwrap();
        let mut out = sup.child_mut().stdout.take().unwrap();
        let mut buf = String::new();
        let read =
            tokio::time::timeout(Duration::from_secs(10), out.read_to_string(&mut buf)).await;
        assert!(
            read.is_ok(),
            "stdout read timed out — child blocked on an undrained stderr pipe"
        );
        assert!(
            buf.contains("DONE"),
            "child never finished; stdout was {buf:?}"
        );
    }

    #[tokio::test]
    async fn stderr_cursor_excludes_older_process_lines() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let mut sup = Supervised::spawn(
            "/bin/sh",
            &[
                "-c",
                "echo old 1>&2; echo READY; read _; echo new-a 1>&2; echo new-b 1>&2; echo DONE",
            ],
            None,
        )
        .unwrap();
        let ring = sup.stderr_ring();
        let stdout = sup.child_mut().stdout.take().unwrap();
        let mut stdout = BufReader::new(stdout).lines();
        assert_eq!(stdout.next_line().await.unwrap().as_deref(), Some("READY"));

        for _ in 0..100 {
            if ring.best_effort_since(ring.origin()).line_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(ring.best_effort_since(ring.origin()).line_count(), 1);
        let cursor = ring.cursor();

        sup.child_mut()
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"continue\n")
            .await
            .unwrap();
        assert_eq!(stdout.next_line().await.unwrap().as_deref(), Some("DONE"));
        for _ in 0..100 {
            if ring.best_effort_since(cursor).line_count() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let snapshot = ring.best_effort_since(cursor);
        assert_eq!(snapshot.line_count(), 2);
        assert_eq!(snapshot.scope(), crate::diagnostics::StderrScope::Process);
        assert_eq!(snapshot.retained_lines(), &["new-a", "new-b"]);
    }

    #[tokio::test]
    async fn stderr_ring_is_bounded_and_debug_omits_text() {
        let mut sup = Supervised::spawn(
            "/bin/sh",
            &[
                "-c",
                "i=0; while [ $i -lt 80 ]; do echo secret-$i 1>&2; i=$((i+1)); done",
            ],
            None,
        )
        .unwrap();
        let ring = sup.stderr_ring();
        let _ = sup.child_mut().wait().await;
        for _ in 0..100 {
            if ring.best_effort_since(ring.origin()).line_count() == 80 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let snapshot = ring.best_effort_since(ring.origin());
        assert_eq!(snapshot.line_count(), 80, "count includes evicted lines");
        assert_eq!(
            snapshot.retained_lines().len(),
            32,
            "text ring stays bounded"
        );
        assert!(!format!("{ring:?}").contains("secret-"));
        assert!(!format!("{snapshot:?}").contains("secret-"));
    }

    #[tokio::test]
    async fn unterminated_oversized_stderr_record_stays_bounded_and_drains() {
        use tokio::io::AsyncReadExt;

        // Regression: `BufRead::lines()` allocates until a newline. This child
        // writes a multi-megabyte record with no delimiter before its stdout
        // sentinel; the drain must stay bounded and let the child finish.
        let mut sup = Supervised::spawn(
            "/bin/sh",
            &["-c", "head -c 2097152 /dev/zero 1>&2; echo DONE"],
            None,
        )
        .unwrap();
        let ring = sup.stderr_ring();
        let mut stdout = sup.child_mut().stdout.take().unwrap();
        let mut text = String::new();
        tokio::time::timeout(Duration::from_secs(10), stdout.read_to_string(&mut text))
            .await
            .expect("bounded stderr drain must not block stdout")
            .unwrap();
        assert_eq!(text.trim(), "DONE");
        let _ = sup.child_mut().wait().await;

        for _ in 0..100 {
            if ring.best_effort_since(ring.origin()).line_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let snapshot = ring.best_effort_since(ring.origin());
        assert_eq!(snapshot.line_count(), 1);
        assert_eq!(snapshot.retained_lines(), &["[REDACTED LINE]"]);
    }

    #[tokio::test]
    async fn stderr_is_sanitized_with_bridge_known_credentials_before_retention() {
        const SECRET: &str = "bridge-known-secret-value";
        let command = format!("echo auth={SECRET} 1>&2");
        let mut sup = Supervised::spawn_with_stderr_redactor(
            "/bin/sh",
            &["-c", &command],
            None,
            DiagnosticRedactor::new([SECRET]),
        )
        .unwrap();
        let ring = sup.stderr_ring();
        let _ = sup.child_mut().wait().await;
        for _ in 0..100 {
            if ring.best_effort_since(ring.origin()).line_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let snapshot = ring.best_effort_since(ring.origin());
        assert_eq!(snapshot.line_count(), 1);
        assert!(!snapshot.retained_lines()[0].contains(SECRET));
        assert!(snapshot.retained_lines()[0].contains("[REDACTED KNOWN SECRET]"));
    }

    #[tokio::test]
    async fn adopted_process_redacts_lines_captured_before_and_after_policy_install() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        const SECRET: &str = "adopted-process-known-secret";
        let mut sup = Supervised::spawn(
            "/bin/sh",
            &[
                "-c",
                "echo adopted-process-known-secret 1>&2; echo READY; read _; \
                 echo adopted-process-known-secret 1>&2; echo DONE",
            ],
            None,
        )
        .unwrap();
        let ring = sup.stderr_ring();
        let stdout = sup.child_mut().stdout.take().unwrap();
        let mut stdout = BufReader::new(stdout).lines();
        assert_eq!(stdout.next_line().await.unwrap().as_deref(), Some("READY"));
        for _ in 0..100 {
            if ring.best_effort_since(ring.origin()).line_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        sup.apply_stderr_redactor(DiagnosticRedactor::new([SECRET]));
        sup.child_mut()
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"continue\n")
            .await
            .unwrap();
        assert_eq!(stdout.next_line().await.unwrap().as_deref(), Some("DONE"));
        let _ = sup.child_mut().wait().await;
        for _ in 0..100 {
            if ring.best_effort_since(ring.origin()).line_count() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let snapshot = ring.best_effort_since(ring.origin());
        assert_eq!(snapshot.line_count(), 2);
        assert!(snapshot
            .retained_lines()
            .iter()
            .all(|line| !line.contains(SECRET) && line.contains("REDACTED KNOWN SECRET")));
    }

    #[tokio::test]
    async fn metadata_only_retention_is_monotonic_across_policy_replacement() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        const BEFORE: &str = "credential-before-metadata-only";
        const AFTER: &str = "credential-after-policy-replacement";
        let mut sup = Supervised::spawn(
            "/bin/sh",
            &[
                "-c",
                "echo credential-before-metadata-only 1>&2; echo READY; read _; \
                 echo credential-after-policy-replacement 1>&2; echo DONE",
            ],
            None,
        )
        .unwrap();
        let ring = sup.stderr_ring();
        let stdout = sup.child_mut().stdout.take().unwrap();
        let mut stdout = BufReader::new(stdout).lines();
        assert_eq!(stdout.next_line().await.unwrap().as_deref(), Some("READY"));
        for _ in 0..100 {
            if ring.best_effort_since(ring.origin()).line_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        ring.retain_metadata_only();
        ring.apply_redactor(DiagnosticRedactor::default());
        assert!(ring.is_metadata_only());
        sup.child_mut()
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"continue\n")
            .await
            .unwrap();
        assert_eq!(stdout.next_line().await.unwrap().as_deref(), Some("DONE"));
        let _ = sup.child_mut().wait().await;
        for _ in 0..100 {
            if ring.best_effort_since(ring.origin()).line_count() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let snapshot = ring.best_effort_since(ring.origin());
        assert_eq!(snapshot.line_count(), 2);
        assert_eq!(
            snapshot.retained_lines(),
            &["[REDACTED LINE]", "[REDACTED LINE]"]
        );
        assert!(snapshot
            .retained_lines()
            .iter()
            .all(|line| !line.contains(BEFORE) && !line.contains(AFTER)));
    }

    #[tokio::test]
    async fn stderr_byte_boundary_and_invalid_utf8_are_bounded_and_valid() {
        let exact = "x".repeat(STDERR_LINE_MAX_BYTES);
        let oversized = "y".repeat(STDERR_LINE_MAX_BYTES + 1);
        let command = format!(
            "printf '%s\\n' '{exact}' 1>&2; printf '%s\\n' '{oversized}' 1>&2; \
             printf '\\377invalid\\n' 1>&2"
        );
        let mut sup = Supervised::spawn("/bin/sh", &["-c", &command], None).unwrap();
        let ring = sup.stderr_ring();
        let _ = sup.child_mut().wait().await;
        for _ in 0..100 {
            if ring.best_effort_since(ring.origin()).line_count() == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let snapshot = ring.best_effort_since(ring.origin());
        assert_eq!(snapshot.line_count(), 3);
        assert_eq!(snapshot.retained_lines()[0].len(), STDERR_LINE_MAX_BYTES);
        assert_eq!(snapshot.retained_lines()[1], "[REDACTED LINE]");
        assert_eq!(snapshot.retained_lines()[2], "�invalid");
        assert!(snapshot.retained_lines()[2].is_char_boundary(0));
    }

    #[tokio::test]
    async fn concurrent_stderr_writers_remain_process_scoped() {
        let mut sup = Supervised::spawn(
            "/bin/sh",
            &[
                "-c",
                "(i=0; while [ $i -lt 10 ]; do echo a-$i 1>&2; i=$((i+1)); done) & \
                 (i=0; while [ $i -lt 10 ]; do echo b-$i 1>&2; i=$((i+1)); done) & wait",
            ],
            None,
        )
        .unwrap();
        let ring = sup.stderr_ring();
        let _ = sup.child_mut().wait().await;
        for _ in 0..100 {
            if ring.best_effort_since(ring.origin()).line_count() == 20 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let snapshot = ring.best_effort_since(ring.origin());
        assert_eq!(snapshot.line_count(), 20);
        assert_eq!(snapshot.scope(), crate::diagnostics::StderrScope::Process);
        assert!(snapshot
            .retained_lines()
            .iter()
            .any(|line| line.starts_with("a-")));
        assert!(snapshot
            .retained_lines()
            .iter()
            .any(|line| line.starts_with("b-")));
    }

    #[tokio::test]
    async fn drop_group_kills_descendants() {
        // The leak fix: a DESCENDANT in the leader's group must die when the Supervised is plain-DROPPED
        // (not terminate()d) — else e.g. codex-acp's spawnSync'd `codex` grandchild orphans and piles up
        // across runs. The leader spawns `sleep 300` in its group + writes its pid; after drop it's gone.
        let dir = std::env::temp_dir().join(format!("a2a-sup-drop-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let pidfile = dir.join("gc.pid");
        let script = format!("sleep 300 & echo $! > {pf}; wait", pf = pidfile.display());
        let sup = Supervised::spawn("/bin/sh", &["-c", &script], None).unwrap();

        // Wait for the grandchild pid to be written.
        let mut gc: Option<u32> = None;
        for _ in 0..100 {
            if let Ok(s) = std::fs::read_to_string(&pidfile) {
                if let Ok(n) = s.trim().parse::<u32>() {
                    gc = Some(n);
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let gc = gc.expect("grandchild pid written");
        assert!(proc_state(gc).is_some(), "grandchild alive before drop");

        drop(sup); // plain drop — must GROUP-kill, not just kill_on_drop the leader

        // The grandchild must be killed (gone, or a zombie pending reap by init) — not left running.
        let mut killed = false;
        for _ in 0..150 {
            match proc_state(gc) {
                None => {
                    killed = true;
                    break;
                }
                Some(st) if st.starts_with('Z') => {
                    killed = true;
                    break;
                }
                _ => {}
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            killed,
            "grandchild must be group-killed on Supervised drop (no orphan); it was still running"
        );
    }

    #[tokio::test]
    async fn terminate_reaps_child_no_zombie() {
        let child = Supervised::spawn("/bin/sh", &["-c", "sleep 30"], None).unwrap();
        let pid = child.pid();
        assert!(proc_state(pid).is_some()); // alive
        child.terminate(Duration::from_millis(300)).await;
        // after terminate+reap, pid is gone (None) — NOT a zombie ('Z' state)
        match proc_state(pid) {
            None => {} // reaped, good
            Some(s) => assert!(!s.starts_with('Z'), "left a zombie: {s}"),
        }
    }

    #[tokio::test]
    async fn term_ignoring_child_with_descendant_is_group_killed() {
        // parent ignores TERM and has a child that sleeps; only a GROUP kill gets both.
        let child = Supervised::spawn(
            "/bin/sh",
            &[
                "-c",
                "trap '' TERM; sleep 30 & echo $! > /tmp/a2a_desc_pid.$$; wait",
            ],
            None,
        )
        .unwrap();
        let pid = child.pid();
        tokio::time::sleep(Duration::from_millis(200)).await; // let descendant spawn
        let desc = std::fs::read_to_string(format!("/tmp/a2a_desc_pid.{pid}"))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        child.terminate(Duration::from_millis(300)).await; // grace then SIGKILL group
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            proc_state(pid).is_none_or(|s| s.starts_with('Z')),
            "leader still running"
        );
        if let Some(d) = desc {
            assert!(
                proc_state(d).is_none(),
                "descendant {d} survived group kill"
            );
        }
        let _ = std::fs::remove_file(format!("/tmp/a2a_desc_pid.{pid}"));
    }

    #[tokio::test]
    async fn term_ignoring_loop_forces_group_sigkill() {
        // sh ignores TERM and re-spawns short sleeps in a loop: group-SIGTERM kills the
        // current `sleep` but the loop (sh ignores TERM) survives the grace window, so only
        // the SIGKILL group escalation can reap it.
        let child = Supervised::spawn(
            "/bin/sh",
            &["-c", "trap '' TERM; while :; do sleep 0.2; done"],
            None,
        )
        .unwrap();
        let pid = child.pid();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            proc_state(pid).is_some(),
            "leader should be alive before terminate"
        );
        // short grace -> SIGTERM ignored by the loop -> timeout -> SIGKILL group
        child.terminate(Duration::from_millis(250)).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            proc_state(pid).is_none(),
            "group SIGKILL should have reaped the TERM-ignoring leader"
        );
    }
}
