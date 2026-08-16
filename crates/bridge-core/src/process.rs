//! Process ownership and signal authority for R2f1b.
//!
//! Design notes:
//! - `OwnedProcessTreeV1` is the sole live capability. It owns the child, immutable
//!   root identity, authority port, one attempt-local registry, and the retained
//!   flight. Raw `Supervised` construction is private; direct uses below are the
//!   sanctioned process-unit seam for stderr and legacy wire-behaviour tests.
//! - `AcpBackend` retains a clone of that capability. Session insertion attaches a
//!   node owner, ordinary release detaches only that owner, and cancel escalation,
//!   driver escalation, registry retirement, and final Drop all clone/join the same
//!   flight; none reconstructs authority from a PID or journal row.
//! - Containment is a stop-and-rescan closure: every known group member is
//!   identity-gated and SIGSTOPed, the group is rescanned until stable, then
//!   immutable members are SIGKILLed child-first and the root last. A child forked
//!   during an early sweep is therefore captured by the next sweep.
//! - `ProcessStartIdentityV1::start_time_ticks` is opaque. Linux stores proc-stat
//!   field 22 (boot-clock ticks); macOS stores KERN_PROC_PID p_starttime (Unix
//!   epoch microseconds). Values are compared only on their originating platform.
//! - Legacy V2 always creates an internal in-memory flight, but its destructive
//!   compatibility branch preserves the historical syscall sequence: explicit
//!   termination is group SIGTERM, bounded wait, then group SIGKILL; final drop is
//!   one immediate group SIGKILL followed by Tokio's direct-child drop backstop.
//!   Protected V3 accepts an attempt-owned registry and durable file journal created
//!   before spawn, uses identity-gated child-first containment, and refuses an
//!   unarmed final drop without any signal. Both attach universally and serialize
//!   teardown through one flight; only V3 makes journal/refusal success a precondition
//!   of destructive dispatch.
//!   Test census: stderr/cwd/raw-wire tests use the private seam; ownership,
//!   identity, ordering, joins, and drop tests use the retained flight.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::{Deref, DerefMut};
use std::process::Stdio;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStderr, Command};

use crate::attempt_activity::SystemMonotonicClock;
use crate::execution_policy::{BoundedCauseV1, Sha256HexV1};
use crate::ids::{AttemptId, NodeId};
use crate::reaper::DurableContainerFlightV3;
use crate::resource_flight::{
    ProcessStartIdentityV1, ResourceActionDispositionV1, ResourceActionResultV1,
    ResourceFlightIdV1, ResourceFlightStateV1, ResourceIdentityV1,
};
pub use crate::retained_resource_flight::OwnedProcessTreeV1;
use crate::retained_resource_flight::{
    FileResourceFlightJournal, InMemoryResourceFlightJournal, NoopResourceFlightResultPublisher,
    ProcessBindingStageV1, ProcessSignalObservationV1, ResourceActionIntentV1,
    ResourceFlightJournal, ResourceFlightKeyV1, ResourceFlightOwnerV1, ResourceFlightRegistryV1,
    ResourceFlightReservationV1, ResourceFlightResultPublisher, RetainedResourceFlight,
    RetainedResourceFlightConfigV1,
};

use crate::diagnostics::{DiagnosticCode, DiagnosticFailureClass, DiagnosticRedactor};

const STDERR_RING_CAPACITY: usize = 32;
const STDERR_LINE_MAX_BYTES: usize = 512;
const BLOCKING_TERMINATE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);
const BLOCKING_TERMINATE_REAP_BOUND: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(target_os = "macos")]
const DARWIN_PS_CENSUS_MAX_BYTES: u64 = 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessStderrCursor {
    line_sequence: u64,
    byte_advance: u64,
}

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
    byte_advance: u64,
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
                "byte_count",
                &state.as_ref().map_or(0, |state| state.byte_advance),
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
        ProcessStderrCursor {
            line_sequence: 0,
            byte_advance: 0,
        }
    }

    pub fn cursor(&self) -> ProcessStderrCursor {
        self.state.lock().map_or(
            ProcessStderrCursor {
                line_sequence: 0,
                byte_advance: 0,
            },
            |state| ProcessStderrCursor {
                line_sequence: state.sequence,
                byte_advance: state.byte_advance,
            },
        )
    }

    fn observe_read(&self, bytes: usize) {
        let mut state = self.state.lock().expect("process stderr ring lock");
        state.byte_advance = state
            .byte_advance
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
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
        let line_count = state
            .sequence
            .saturating_sub(cursor.line_sequence)
            .min(u32::MAX as u64) as u32;
        let byte_count = state.byte_advance.saturating_sub(cursor.byte_advance);
        let retained_lines = if include_retained_text {
            state
                .lines
                .iter()
                .filter(|line| line.sequence > cursor.line_sequence)
                .map(|line| line.text.clone())
                .collect()
        } else {
            Vec::new()
        };
        ProcessStderrSnapshot {
            line_count,
            byte_count,
            retained_lines,
        }
    }
}

#[derive(Clone)]
pub struct ProcessStderrSnapshot {
    line_count: u32,
    byte_count: u64,
    retained_lines: Vec<String>,
}

impl std::fmt::Debug for ProcessStderrSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessStderrSnapshot")
            .field("line_count", &self.line_count)
            .field("byte_count", &self.byte_count)
            .field("scope", &crate::diagnostics::StderrScope::Process)
            .field("retained_line_count", &self.retained_lines.len())
            .finish()
    }
}

impl ProcessStderrSnapshot {
    pub fn line_count(&self) -> u32 {
        self.line_count
    }

    pub fn byte_count(&self) -> u64 {
        self.byte_count
    }

    pub fn scope(&self) -> crate::diagnostics::StderrScope {
        crate::diagnostics::StderrScope::Process
    }

    /// Bounded, already-redacted lines. Metadata-only snapshots return an empty slice.
    pub fn retained_lines(&self) -> &[String] {
        &self.retained_lines
    }
}

pub(crate) struct Supervised {
    // Raw supervision exists only as the narrow process-module test seam. Live
    // production values set this false and are signaled only by their flight.
    #[cfg_attr(not(test), allow(dead_code))]
    drop_signal: bool,
    child: Child,
    pid: u32,
    stderr_ring: ProcessStderrRing,
}

impl Supervised {
    #[cfg(test)]
    pub fn spawn(
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
    ) -> std::io::Result<Self> {
        Self::spawn_with_stderr_redactor(prog, args, cwd, DiagnosticRedactor::default())
    }

    /// Spawn a supervised child and sanitize process stderr with the supplied
    /// bridge-known credential set before any text enters the bounded ring.
    #[cfg(test)]
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
    #[cfg(test)]
    pub fn spawn_with_stderr_redactor_and_pinned_cwd(
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
        stderr_redactor: DiagnosticRedactor,
        pinned_cwd_fd: Option<std::os::fd::RawFd>,
        retain_pinned_cwd_fd_after_exec: bool,
    ) -> std::io::Result<Self> {
        Self::spawn_with_stderr_redactor_pinned_cwd_and_env_removals(
            prog,
            args,
            cwd,
            stderr_redactor,
            pinned_cwd_fd,
            retain_pinned_cwd_fd_after_exec,
            &[],
        )
    }

    /// Spawn with the same cwd and stderr guarantees as
    /// [`Self::spawn_with_stderr_redactor_and_pinned_cwd`], while removing the
    /// named variables from the child's inherited environment before exec.
    #[cfg(test)]
    pub fn spawn_with_stderr_redactor_pinned_cwd_and_env_removals(
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
        stderr_redactor: DiagnosticRedactor,
        pinned_cwd_fd: Option<std::os::fd::RawFd>,
        retain_pinned_cwd_fd_after_exec: bool,
        removed_environment: &[String],
    ) -> std::io::Result<Self> {
        Self::spawn_impl(
            prog,
            args,
            cwd,
            stderr_redactor,
            pinned_cwd_fd,
            retain_pinned_cwd_fd_after_exec,
            removed_environment,
            true,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_owned(
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
        stderr_redactor: DiagnosticRedactor,
        pinned_cwd_fd: Option<std::os::fd::RawFd>,
        retain_pinned_cwd_fd_after_exec: bool,
        removed_environment: &[String],
        kill_child_on_drop: bool,
    ) -> std::io::Result<Self> {
        Self::spawn_impl(
            prog,
            args,
            cwd,
            stderr_redactor,
            pinned_cwd_fd,
            retain_pinned_cwd_fd_after_exec,
            removed_environment,
            false,
            kill_child_on_drop,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_impl(
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
        stderr_redactor: DiagnosticRedactor,
        pinned_cwd_fd: Option<std::os::fd::RawFd>,
        retain_pinned_cwd_fd_after_exec: bool,
        removed_environment: &[String],
        drop_signal: bool,
        kill_child_on_drop: bool,
    ) -> std::io::Result<Self> {
        let mut cmd = Command::new(prog);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0) // child becomes its own group leader (pgid == child pid)
            .kill_on_drop(kill_child_on_drop);
        for variable in removed_environment {
            cmd.env_remove(variable);
        }
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
            drop_signal,
            child,
            pid,
            stderr_ring,
        })
    }

    #[cfg(test)]
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

    /// The raw process-unit seam reads stdout/stdin through this.
    #[cfg(test)]
    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// SIGTERM the group, wait `grace`, SIGKILL the group, reap.
    #[cfg(test)]
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

    /// Runtime-independent bounded termination for an owner dropped while its Tokio runtime is
    /// shutting down. This uses only process-group signals and synchronous `try_wait`, so the caller
    /// may run it on a dedicated OS thread after the source reactor has disappeared.
    #[doc(hidden)]
    #[cfg(test)]
    pub fn terminate_blocking(mut self, grace: std::time::Duration) {
        let pgid = self.pid as i32;
        // SAFETY: kill(-pgid, sig) sends a signal to the process group created and owned by this
        // supervisor (`pgid == child pid`).
        unsafe {
            libc::kill(-pgid, libc::SIGTERM);
        }
        let grace_deadline = std::time::Instant::now()
            .checked_add(grace)
            .unwrap_or_else(std::time::Instant::now);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Err(_) => break,
                Ok(None) => {}
            }
            let now = std::time::Instant::now();
            if now >= grace_deadline {
                break;
            }
            std::thread::sleep(std::cmp::min(
                BLOCKING_TERMINATE_POLL_INTERVAL,
                grace_deadline.saturating_duration_since(now),
            ));
        }

        // SAFETY: same owned process group as above. SIGKILL cannot be ignored.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
        let reap_deadline = std::time::Instant::now()
            .checked_add(BLOCKING_TERMINATE_REAP_BOUND)
            .unwrap_or_else(std::time::Instant::now);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => {}
            }
            let now = std::time::Instant::now();
            if now >= reap_deadline {
                return;
            }
            std::thread::sleep(std::cmp::min(
                BLOCKING_TERMINATE_POLL_INTERVAL,
                reap_deadline.saturating_duration_since(now),
            ));
        }
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

        // Advance as soon as bytes cross the owned child pipe. Waiting for a
        // newline or EOF can otherwise make a live, chatty child look idle.
        ring.observe_read(read);

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
    /// The private process-module test seam preserves the pre-flight group-drop
    /// control. Production raw values always carry `drop_signal = false`; their
    /// containing `OwnedProcessTreeV1` drives and settles the retained flight.
    fn drop(&mut self) {
        #[cfg(test)]
        if self.drop_signal {
            // SAFETY: kill(-pgid, SIGKILL) targets a process group we own — `pgid == self.pid`, the group
            // leader, because we spawned with `process_group(0)`.
            unsafe {
                libc::kill(-(self.pid as i32), libc::SIGKILL);
            }
        }
    }
}

// ── Retained process authority ────────────────────────────────────────────────

/// Compatibility gating for process destruction. V2 preserves the historical
/// lifecycle (including synchronous final-drop reaping) through an in-memory
/// flight. V3 requires an explicit flight action; an unarmed final drop records
/// a refusal and performs no signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessFlightModeV1 {
    LegacyV2,
    ProtectedV3,
}

/// Attempt-owned durable inputs for one protected V3 ACP process generation.
///
/// The caller creates one [ResourceFlightRegistryV1] for the attempt and one
/// [FileResourceFlightJournal] under the attempt's pinned persistence root,
/// then moves this value into the spawn call. The concrete file-journal type
/// prevents a V3 caller from silently substituting an in-memory journal.
pub struct DurableProcessFlightV3 {
    attempt_id: AttemptId,
    generation: String,
    flight_id: ResourceFlightIdV1,
    registry: Arc<ResourceFlightRegistryV1>,
    journal: Arc<FileResourceFlightJournal>,
    owner: ResourceFlightOwnerV1,
    result_publisher: Arc<dyn ResourceFlightResultPublisher>,
}

impl std::fmt::Debug for DurableProcessFlightV3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DurableProcessFlightV3")
            .field("attempt_id", &self.attempt_id)
            .field("generation", &self.generation)
            .field("flight_id", &self.flight_id)
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl DurableProcessFlightV3 {
    pub fn new(
        attempt_id: AttemptId,
        generation: impl Into<String>,
        registry: Arc<ResourceFlightRegistryV1>,
        journal: Arc<FileResourceFlightJournal>,
        owner: ResourceFlightOwnerV1,
    ) -> Result<Self, ProcessAuthorityErrorV1> {
        let generation = generation.into();
        if generation.is_empty() {
            return Err(ProcessAuthorityErrorV1::Flight(
                "V3 process generation is empty".into(),
            ));
        }
        Ok(Self {
            attempt_id,
            generation,
            flight_id: ResourceFlightIdV1::mint()
                .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))?,
            registry,
            journal,
            owner,
            result_publisher: Arc::new(NoopResourceFlightResultPublisher),
        })
    }

    #[must_use]
    pub fn with_result_publisher(
        mut self,
        result_publisher: Arc<dyn ResourceFlightResultPublisher>,
    ) -> Self {
        self.result_publisher = result_publisher;
        self
    }

    #[must_use]
    pub fn flight_id(&self) -> &ResourceFlightIdV1 {
        &self.flight_id
    }
}

/// Attempt-scoped production binding for protected ACP generations.
///
/// Construct this exactly once at V3 attempt admission and reuse it for every
/// ACP generation in that attempt. It owns the one in-process registry and the
/// one durable file-journal handle required by the 3a recovery contract; a
/// generation binding can therefore never mint a second registry accidentally.
/// The upstream `AutomaticR2f1b` admission hop remains intentionally refused
/// until slice 4, but its eventual consumer must own one of these values rather
/// than assembling `DurableProcessFlightV3` ad hoc.
pub struct DurableProcessFlightAttemptV3 {
    attempt_id: AttemptId,
    registry: Arc<ResourceFlightRegistryV1>,
    journal: Arc<FileResourceFlightJournal>,
    result_publisher: Arc<dyn ResourceFlightResultPublisher>,
}

impl std::fmt::Debug for DurableProcessFlightAttemptV3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DurableProcessFlightAttemptV3")
            .field("attempt_id", &self.attempt_id)
            .finish_non_exhaustive()
    }
}

impl DurableProcessFlightAttemptV3 {
    #[must_use]
    pub fn new(attempt_id: AttemptId, journal: Arc<FileResourceFlightJournal>) -> Self {
        Self {
            registry: Arc::new(ResourceFlightRegistryV1::new(attempt_id.clone())),
            attempt_id,
            journal,
            result_publisher: Arc::new(NoopResourceFlightResultPublisher),
        }
    }

    #[must_use]
    pub fn with_result_publisher(
        mut self,
        result_publisher: Arc<dyn ResourceFlightResultPublisher>,
    ) -> Self {
        self.result_publisher = result_publisher;
        self
    }

    pub fn bind_generation(
        &self,
        generation: impl Into<String>,
        owner: ResourceFlightOwnerV1,
    ) -> Result<DurableProcessFlightV3, ProcessAuthorityErrorV1> {
        DurableProcessFlightV3::new(
            self.attempt_id.clone(),
            generation,
            Arc::clone(&self.registry),
            Arc::clone(&self.journal),
            owner,
        )
        .map(|flight| flight.with_result_publisher(Arc::clone(&self.result_publisher)))
    }

    /// Bind a container generation to the same attempt registry and durable
    /// journal used by its subordinate ACP process generation.
    pub fn bind_container_generation(
        &self,
        generation: impl Into<String>,
        owner: ResourceFlightOwnerV1,
    ) -> Result<DurableContainerFlightV3, ProcessAuthorityErrorV1> {
        let generation = generation.into();
        if generation.is_empty() {
            return Err(ProcessAuthorityErrorV1::Flight(
                "V3 container generation is empty".into(),
            ));
        }
        Ok(DurableContainerFlightV3 {
            attempt_id: self.attempt_id.clone(),
            generation,
            flight_id: ResourceFlightIdV1::mint()
                .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))?,
            registry: Arc::clone(&self.registry),
            journal: Arc::clone(&self.journal) as Arc<dyn ResourceFlightJournal>,
            owner,
            result_publisher: Arc::clone(&self.result_publisher),
        })
    }
}

enum ProcessFlightProvisionV1 {
    LegacyV2,
    DurableV3(DurableProcessFlightV3),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProcessMemberV1 {
    pub identity: ProcessStartIdentityV1,
    pub parent_pid: u32,
    pub pgid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProcessSyscallOutcomeV1 {
    pub return_code: i32,
    pub errno: Option<i32>,
}

/// Injectable boundary for immutable identity probes, group enumeration, and
/// the final signal syscall. Adapters never receive a bare PID-signaling API:
/// every call below is preceded by an exact start-identity comparison.
pub(crate) trait ProcessAuthorityPortV1: Send + Sync {
    fn immutable_start(&self, pid: u32) -> std::io::Result<ProcessStartIdentityV1>;
    fn process_group_members(&self, pgid: u32) -> std::io::Result<Vec<ProcessMemberV1>>;
    /// Verify that this exact immutable process generation is unable to fork:
    /// either it completed SIGSTOP or it is already a zombie. A recycled
    /// numeric pid is never reported contained for the captured generation.
    fn process_is_stopped(&self, expected: &ProcessStartIdentityV1) -> std::io::Result<bool>;
    fn signal_process(&self, pid: u32, signal: i32) -> ProcessSyscallOutcomeV1;
    /// Compatibility-only V2 group signal. Protected V3 never calls this port:
    /// it signals individually after immutable identity comparison.
    fn signal_process_group(&self, pgid: u32, signal: i32) -> ProcessSyscallOutcomeV1;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirectChildWaitStatusV1 {
    Running,
    Reaped,
}

trait DirectChildWaitPortV1: Send + Sync {
    fn try_wait(&self, child: Option<&mut Supervised>) -> std::io::Result<DirectChildWaitStatusV1>;
}

#[derive(Default)]
struct SystemDirectChildWaitV1;
impl DirectChildWaitPortV1 for SystemDirectChildWaitV1 {
    fn try_wait(&self, child: Option<&mut Supervised>) -> std::io::Result<DirectChildWaitStatusV1> {
        let child = child.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "owned child unavailable")
        })?;
        child.child.try_wait().map(|status| {
            if status.is_some() {
                DirectChildWaitStatusV1::Reaped
            } else {
                DirectChildWaitStatusV1::Running
            }
        })
    }
}

trait ProcessSpawnPortV1: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    fn spawn_owned(
        &self,
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
        stderr_redactor: DiagnosticRedactor,
        pinned_cwd_fd: Option<std::os::fd::RawFd>,
        retain_pinned_cwd_fd_after_exec: bool,
        removed_environment: &[String],
        kill_child_on_drop: bool,
    ) -> std::io::Result<Supervised>;
}

#[derive(Default)]
struct SystemProcessSpawnV1;
impl ProcessSpawnPortV1 for SystemProcessSpawnV1 {
    fn spawn_owned(
        &self,
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
        stderr_redactor: DiagnosticRedactor,
        pinned_cwd_fd: Option<std::os::fd::RawFd>,
        retain_pinned_cwd_fd_after_exec: bool,
        removed_environment: &[String],
        kill_child_on_drop: bool,
    ) -> std::io::Result<Supervised> {
        Supervised::spawn_owned(
            prog,
            args,
            cwd,
            stderr_redactor,
            pinned_cwd_fd,
            retain_pinned_cwd_fd_after_exec,
            removed_environment,
            kill_child_on_drop,
        )
    }
}

#[derive(Default)]
pub(crate) struct SystemProcessAuthorityV1;

/// `KERN_PROC_PID` returns `struct kinfo_proc`, whose first member is
/// `extern_proc`. Darwin's public ABI overlays `p_starttime` with the two
/// legacy queue pointers at offset zero. The pinned libc does not expose
/// `kinfo_proc`, so describe only the prefix we read and let sysctl report the
/// full output size before allocating its opaque trailing bytes.
#[cfg(target_os = "macos")]
#[repr(C)]
struct DarwinExternProcStartPrefixV1 {
    p_starttime: libc::timeval,
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct DarwinKinfoProcStartPrefixV1 {
    kp_proc: DarwinExternProcStartPrefixV1,
}

impl SystemProcessAuthorityV1 {
    #[cfg(target_os = "linux")]
    fn linux_stat(pid: u32) -> std::io::Result<(char, u32, u32, u64)> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let (_, fields) = stat.rsplit_once(") ").ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "malformed /proc stat")
        })?;
        let fields: Vec<_> = fields.split_whitespace().collect();
        if fields.len() <= 19 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "short /proc stat",
            ));
        }
        let parse = |index: usize| {
            fields[index].parse::<u64>().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid /proc stat field")
            })
        };
        let state = fields[0].chars().next().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "empty /proc stat state")
        })?;
        let ppid = u32::try_from(parse(1)?)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "ppid overflow"))?;
        let pgid = u32::try_from(parse(2)?)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "pgid overflow"))?;
        // Linux /proc/<pid>/stat field 22: clock ticks since boot. This is an
        // opaque, same-platform generation token; it is never converted to a
        // wall clock or compared with the Darwin representation.
        Ok((state, ppid, pgid, parse(19)?))
    }

    #[cfg(target_os = "macos")]
    fn darwin_start(pid: u32) -> std::io::Result<u64> {
        let mut mib = [
            libc::CTL_KERN,
            libc::KERN_PROC,
            libc::KERN_PROC_PID,
            i32::try_from(pid).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "pid overflow")
            })?,
        ];
        let mut size = 0_usize;
        // SAFETY: a null output pointer asks sysctl for the complete opaque
        // kinfo_proc size and does not write through either null pointer.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as u32,
                std::ptr::null_mut(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc == -1 {
            return Err(std::io::Error::last_os_error());
        }
        if size == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "process not found",
            ));
        }
        if size < std::mem::size_of::<DarwinKinfoProcStartPrefixV1>() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "short KERN_PROC_PID result",
            ));
        }
        let mut info = vec![0_u8; size];
        // SAFETY: `info` is writable for the exact size returned above. The
        // local prefix is read unaligned below because Vec<u8> has byte
        // alignment; sysctl owns no pointer after this call returns.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as u32,
                info.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc == -1 {
            return Err(std::io::Error::last_os_error());
        }
        if size < std::mem::size_of::<DarwinKinfoProcStartPrefixV1>() {
            return Err(std::io::Error::new(
                if size == 0 {
                    std::io::ErrorKind::NotFound
                } else {
                    std::io::ErrorKind::InvalidData
                },
                "short KERN_PROC_PID result",
            ));
        }
        // SAFETY: the length check above proves the complete local prefix was
        // initialized; read_unaligned matches Vec<u8>'s byte alignment.
        let started = unsafe {
            std::ptr::read_unaligned(info.as_ptr().cast::<DarwinKinfoProcStartPrefixV1>())
        }
        .kp_proc
        .p_starttime;
        let seconds = u64::try_from(started.tv_sec).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "negative start second")
        })?;
        let micros = u64::try_from(started.tv_usec).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "negative start microsecond",
            )
        })?;
        // Darwin p_starttime: microseconds since Unix epoch. Like Linux ticks,
        // this is an opaque platform-native token and has no cross-platform
        // ordering or conversion semantics.
        seconds
            .checked_mul(1_000_000)
            .and_then(|value: u64| value.checked_add(micros))
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "start time overflow")
            })
    }
    #[cfg(target_os = "macos")]
    fn darwin_bsd_info(pid: u32) -> std::io::Result<libc::proc_bsdinfo> {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let buffer_size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>())
            .map_err(|_| std::io::Error::other("proc_bsdinfo size overflow"))?;
        // SAFETY: info and buffer_size describe writable storage for exactly
        // one PROC_PIDTBSDINFO result.
        let read = unsafe {
            libc::proc_pidinfo(
                i32::try_from(pid).map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "pid overflow")
                })?,
                libc::PROC_PIDTBSDINFO,
                0,
                std::ptr::addr_of_mut!(info).cast(),
                buffer_size,
            )
        };
        if read <= 0 {
            return Err(std::io::Error::last_os_error());
        }
        if read < buffer_size || info.pbi_pid != pid {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "short or mismatched PROC_PIDTBSDINFO result",
            ));
        }
        Ok(info)
    }
}

impl ProcessAuthorityPortV1 for SystemProcessAuthorityV1 {
    fn immutable_start(&self, pid: u32) -> std::io::Result<ProcessStartIdentityV1> {
        #[cfg(target_os = "linux")]
        let start_time_ticks = Self::linux_stat(pid)?.3;
        #[cfg(target_os = "macos")]
        let start_time_ticks = Self::darwin_start(pid)?;
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let start_time_ticks = {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "immutable process identity is unsupported",
            ));
        };
        Ok(ProcessStartIdentityV1 {
            pid,
            start_time_ticks,
            executable_sha256: None,
        })
    }

    fn process_group_members(&self, pgid: u32) -> std::io::Result<Vec<ProcessMemberV1>> {
        #[cfg(target_os = "linux")]
        {
            let mut members = Vec::new();
            for entry in std::fs::read_dir("/proc")? {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => continue,
                };
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|value| value.parse::<u32>().ok())
                else {
                    continue;
                };
                let Ok((_, parent_pid, observed_pgid, start_time_ticks)) = Self::linux_stat(pid)
                else {
                    continue;
                };
                if observed_pgid == pgid {
                    members.push(ProcessMemberV1 {
                        identity: ProcessStartIdentityV1 {
                            pid,
                            start_time_ticks,
                            executable_sha256: None,
                        },
                        parent_pid,
                        pgid: observed_pgid,
                    });
                }
            }
            Ok(members)
        }
        #[cfg(target_os = "macos")]
        {
            let mut child = std::process::Command::new("/bin/ps")
                .args(["-axo", "pid=,ppid=,pgid="])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| std::io::Error::other("ps stdout unavailable"))?;
            let reader = std::thread::spawn(move || {
                let mut output = Vec::new();
                let mut bounded =
                    std::io::Read::take(stdout, DARWIN_PS_CENSUS_MAX_BYTES.saturating_add(1));
                std::io::Read::read_to_end(&mut bounded, &mut output).map(|_| output)
            });
            let deadline = std::time::Instant::now()
                .checked_add(std::time::Duration::from_secs(1))
                .unwrap_or_else(std::time::Instant::now);
            let status = loop {
                match child.try_wait() {
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = reader.join();
                        return Err(error);
                    }
                    Ok(Some(status)) => break status,
                    Ok(None) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Ok(None) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = reader.join();
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "/bin/ps process-group census timed out",
                        ));
                    }
                }
            };
            let output = reader
                .join()
                .map_err(|_| std::io::Error::other("ps census reader panicked"))??;
            if u64::try_from(output.len()).unwrap_or(u64::MAX) > DARWIN_PS_CENSUS_MAX_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "/bin/ps process-group census output exceeded its byte bound",
                ));
            }
            if !status.success() {
                return Err(std::io::Error::other("ps process-group census failed"));
            }
            let mut members = Vec::new();
            for line in String::from_utf8_lossy(&output).lines() {
                let fields: Vec<_> = line.split_whitespace().collect();
                if fields.len() != 3 {
                    continue;
                }
                let (Ok(pid), Ok(parent_pid), Ok(enumerated_pgid)) = (
                    fields[0].parse::<u32>(),
                    fields[1].parse::<u32>(),
                    fields[2].parse::<u32>(),
                ) else {
                    continue;
                };
                if enumerated_pgid != pgid {
                    continue;
                }
                let Ok(before) = self.immutable_start(pid) else {
                    continue;
                };
                let Ok(raw_pid) = i32::try_from(pid) else {
                    continue;
                };
                // SAFETY: getpgid only queries the numeric candidate. Both
                // immutable-start probes below bind the result to one process
                // generation before the candidate can be admitted.
                let kernel_pgid = unsafe { libc::getpgid(raw_pid) };
                let Ok(info) = Self::darwin_bsd_info(pid) else {
                    continue;
                };
                let Ok(after) = self.immutable_start(pid) else {
                    continue;
                };
                if before != after
                    || kernel_pgid < 0
                    || u32::try_from(kernel_pgid).ok() != Some(pgid)
                    || info.pbi_ppid != parent_pid
                {
                    continue;
                }
                if members.len() == MAX_PROCESS_GROUP_MEMBERS {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "process group member bound exceeded",
                    ));
                }
                members.push(ProcessMemberV1 {
                    identity: after,
                    parent_pid: info.pbi_ppid,
                    pgid,
                });
            }
            Ok(members)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "process-group census is unsupported",
        ))
    }

    fn process_is_stopped(&self, expected: &ProcessStartIdentityV1) -> std::io::Result<bool> {
        #[cfg(target_os = "linux")]
        {
            let (state, _, _, start_time_ticks) = Self::linux_stat(expected.pid)?;
            Ok(start_time_ticks == expected.start_time_ticks
                && expected.executable_sha256.is_none()
                && matches!(state, 'T' | 'Z'))
        }
        #[cfg(target_os = "macos")]
        {
            let before = Self::darwin_start(expected.pid)?;
            if before != expected.start_time_ticks || expected.executable_sha256.is_some() {
                return Ok(false);
            }
            let info = Self::darwin_bsd_info(expected.pid)?;
            // Re-probe immutable identity after the status read so a recycle
            // between the two platform probes cannot verify the old member.
            let after = Self::darwin_start(expected.pid)?;
            Ok(before == after
                && after == expected.start_time_ticks
                && info.pbi_pid == expected.pid
                && matches!(info.pbi_status, libc::SSTOP | libc::SZOMB))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = expected;
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "process stopped-state probe is unsupported",
            ))
        }
    }

    fn signal_process(&self, pid: u32, signal: i32) -> ProcessSyscallOutcomeV1 {
        let pid = match i32::try_from(pid) {
            Ok(pid) => pid,
            Err(_) => {
                return ProcessSyscallOutcomeV1 {
                    return_code: -1,
                    errno: Some(libc::EINVAL),
                }
            }
        };
        // SAFETY: the immutable-identity gate immediately before this call has
        // proven the numeric pid still denotes the captured process generation.
        let return_code = unsafe { libc::kill(pid, signal) };
        ProcessSyscallOutcomeV1 {
            return_code,
            errno: (return_code == -1)
                .then(|| std::io::Error::last_os_error().raw_os_error())
                .flatten(),
        }
    }

    fn signal_process_group(&self, pgid: u32, signal: i32) -> ProcessSyscallOutcomeV1 {
        let pgid = match i32::try_from(pgid) {
            Ok(pgid) => pgid,
            Err(_) => {
                return ProcessSyscallOutcomeV1 {
                    return_code: -1,
                    errno: Some(libc::EINVAL),
                }
            }
        };
        // SAFETY: Legacy V2 intentionally preserves the pre-flight process-group
        // syscall exactly. The group was created by this supervisor and pgid is
        // the captured leader pid. Protected V3 cannot reach this method.
        let return_code = unsafe { libc::kill(-pgid, signal) };
        ProcessSyscallOutcomeV1 {
            return_code,
            errno: (return_code == -1)
                .then(|| std::io::Error::last_os_error().raw_os_error())
                .flatten(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessAuthorityErrorV1 {
    Flight(String),
    IdentityUnavailable { pid: u32, detail: String },
    IdentityChanged { pid: u32 },
    ProcessUnavailable,
    BareJoinRefused,
    ContainmentUnstable,
    CensusUnavailable { detail: String },
    ReapTimeout { pid: u32 },
}

impl std::fmt::Display for ProcessAuthorityErrorV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Flight(detail) => write!(f, "process resource flight refused: {detail}"),
            Self::IdentityUnavailable { pid, detail } => {
                write!(f, "immutable identity unavailable for pid {pid}: {detail}")
            }
            Self::IdentityChanged { pid } => {
                write!(f, "pid {pid} no longer denotes the captured process")
            }
            Self::ProcessUnavailable => f.write_str("owned process handle is unavailable"),
            Self::BareJoinRefused => f.write_str("no process resource flight is active to join"),
            Self::ContainmentUnstable => {
                f.write_str("process tree did not reach a stopped, stable closure")
            }
            Self::CensusUnavailable { detail } => {
                write!(f, "process-group census unavailable: {detail}")
            }
            Self::ReapTimeout { pid } => {
                write!(f, "direct child {pid} did not reap before deadline")
            }
        }
    }
}
impl std::error::Error for ProcessAuthorityErrorV1 {}

#[derive(Clone)]
enum OwnedActionStateV1 {
    Idle,
    Driving,
    Finished(Result<ResourceActionResultV1, ProcessAuthorityErrorV1>),
}

enum ActionClaimV1 {
    Drive,
    Join,
    Finished,
}

const CONTAINMENT_CLOSURE_LIMIT: usize = 16;
const MAX_PROCESS_GROUP_MEMBERS: usize = 512;

pub(crate) struct OwnedProcessStateV1 {
    identity: ResourceIdentityV1,
    child_wait: Arc<dyn DirectChildWaitPortV1>,
    child: Mutex<Option<Supervised>>,
    authority: Arc<dyn ProcessAuthorityPortV1>,
    descendant_registry: Mutex<BTreeMap<u32, ProcessMemberV1>>,
    flight: Arc<RetainedResourceFlight>,
    _registry: Arc<ResourceFlightRegistryV1>,
    journal: Arc<dyn ResourceFlightJournal>,
    owner: ResourceFlightOwnerV1,
    reap_bound: std::time::Duration,
    mode: ProcessFlightModeV1,
    started: std::time::Instant,
    action: Mutex<OwnedActionStateV1>,
    action_settled: Condvar,
}

struct DrivingActionGuardV1<'a> {
    state: &'a OwnedProcessStateV1,
    armed: bool,
}

impl<'a> DrivingActionGuardV1<'a> {
    fn new(state: &'a OwnedProcessStateV1) -> Self {
        Self { state, armed: true }
    }

    fn finish(
        mut self,
        result: Result<ResourceActionResultV1, ProcessAuthorityErrorV1>,
    ) -> Result<ResourceActionResultV1, ProcessAuthorityErrorV1> {
        let result = self.state.finish_action(result);
        self.armed = false;
        result
    }
}

impl Drop for DrivingActionGuardV1<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let result = Err(ProcessAuthorityErrorV1::Flight(
            "process action driver unwound before settlement".into(),
        ));
        let mut action = self
            .state
            .action
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*action, OwnedActionStateV1::Driving) {
            *action = OwnedActionStateV1::Finished(result);
            self.state.action_settled.notify_all();
        }
    }
}
pub struct OwnedChildGuardV1<'a> {
    guard: MutexGuard<'a, Option<Supervised>>,
}
impl Deref for OwnedChildGuardV1<'_> {
    type Target = Child;
    fn deref(&self) -> &Self::Target {
        &self
            .guard
            .as_ref()
            .expect("owned process is available")
            .child
    }
}
impl DerefMut for OwnedChildGuardV1<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self
            .guard
            .as_mut()
            .expect("owned process is available")
            .child
    }
}

impl OwnedProcessStateV1 {
    fn root_identity(&self) -> &ProcessStartIdentityV1 {
        match &self.identity {
            ResourceIdentityV1::AcpProcess {
                immutable_start, ..
            } => immutable_start,
            _ => unreachable!("owned process state always has an ACP identity"),
        }
    }

    fn pgid(&self) -> u32 {
        match &self.identity {
            ResourceIdentityV1::AcpProcess { pgid, pid, .. } => pgid.unwrap_or(*pid),
            _ => unreachable!("owned process state always has an ACP identity"),
        }
    }

    fn claim_action(&self) -> Result<ActionClaimV1, ProcessAuthorityErrorV1> {
        let mut action = self
            .action
            .lock()
            .map_err(|_| ProcessAuthorityErrorV1::Flight("action lock poisoned".into()))?;
        Ok(match &*action {
            OwnedActionStateV1::Idle => {
                *action = OwnedActionStateV1::Driving;
                ActionClaimV1::Drive
            }
            OwnedActionStateV1::Driving => ActionClaimV1::Join,
            OwnedActionStateV1::Finished(_) => ActionClaimV1::Finished,
        })
    }

    fn finish_action(
        &self,
        result: Result<ResourceActionResultV1, ProcessAuthorityErrorV1>,
    ) -> Result<ResourceActionResultV1, ProcessAuthorityErrorV1> {
        let mut action = self
            .action
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *action = OwnedActionStateV1::Finished(result.clone());
        self.action_settled.notify_all();
        result
    }

    fn join_blocking(&self) -> Result<ResourceActionResultV1, ProcessAuthorityErrorV1> {
        self.reject_recycled_pid_if_present()?;
        let mut action = self
            .action
            .lock()
            .map_err(|_| ProcessAuthorityErrorV1::Flight("action lock poisoned".into()))?;
        loop {
            match &*action {
                OwnedActionStateV1::Idle => return Err(ProcessAuthorityErrorV1::BareJoinRefused),
                OwnedActionStateV1::Driving => {
                    action = self.action_settled.wait(action).map_err(|_| {
                        ProcessAuthorityErrorV1::Flight("action join lock poisoned".into())
                    })?;
                }
                OwnedActionStateV1::Finished(Err(error)) => return Err(error.clone()),
                OwnedActionStateV1::Finished(Ok(_)) => {
                    drop(action);
                    return self
                        .flight
                        .join_blocking()
                        .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()));
                }
            }
        }
    }

    fn reject_recycled_pid_if_present(&self) -> Result<(), ProcessAuthorityErrorV1> {
        let expected = self.root_identity();
        match self.authority.immutable_start(expected.pid) {
            Ok(current) if current == *expected => Ok(()),
            Ok(_) => Err(ProcessAuthorityErrorV1::IdentityChanged { pid: expected.pid }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let joinable = self.action.lock().is_ok_and(|state| {
                    matches!(
                        *state,
                        OwnedActionStateV1::Driving | OwnedActionStateV1::Finished(_)
                    )
                });
                if joinable {
                    Ok(())
                } else {
                    Err(ProcessAuthorityErrorV1::IdentityUnavailable {
                        pid: expected.pid,
                        detail: error.to_string(),
                    })
                }
            }
            Err(error) => Err(ProcessAuthorityErrorV1::IdentityUnavailable {
                pid: expected.pid,
                detail: error.to_string(),
            }),
        }
    }

    fn prepare_dispatch(&self) -> Result<(), ProcessAuthorityErrorV1> {
        self.flight
            .close_admission()
            .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))?;
        self.flight
            .journal_intent(ResourceActionIntentV1 {
                initiator: self.owner.clone(),
                capability_digest: Sha256HexV1::digest(
                    serde_json::to_vec(&self.identity)
                        .unwrap_or_else(|_| b"owned-process-tree-v1".to_vec())
                        .as_slice(),
                ),
                cause: None,
            })
            .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))?;
        self.flight
            .begin_journaled_dispatch()
            .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))?;
        Ok(())
    }

    fn observation_for(
        &self,
        expected: &ProcessStartIdentityV1,
        signal: i32,
    ) -> ProcessSignalObservationV1 {
        let outcome = match self.authority.immutable_start(expected.pid) {
            Ok(current) if current == *expected => {
                self.authority.signal_process(expected.pid, signal)
            }
            Ok(_) => ProcessSyscallOutcomeV1 {
                return_code: -1,
                errno: Some(libc::ESTALE),
            },
            Err(error) => ProcessSyscallOutcomeV1 {
                return_code: -1,
                errno: Some(error.raw_os_error().unwrap_or(libc::ESRCH)),
            },
        };
        ProcessSignalObservationV1 {
            pid: expected.pid,
            expected_start_time_ticks: expected.start_time_ticks,
            signal,
            return_code: outcome.return_code,
            errno: outcome.errno,
        }
    }

    fn legacy_v2_group_observation(&self, signal: i32) -> ProcessSignalObservationV1 {
        let outcome = self.authority.signal_process_group(self.pgid(), signal);
        ProcessSignalObservationV1 {
            // V2's historical target is -pgid. The evidence carries that
            // positive captured group leader in the existing pid-shaped field.
            pid: self.pgid(),
            expected_start_time_ticks: self.root_identity().start_time_ticks,
            signal,
            return_code: outcome.return_code,
            errno: outcome.errno,
        }
    }

    fn members_with_live_root(&self) -> std::io::Result<Vec<ProcessMemberV1>> {
        let candidates: BTreeMap<u32, ProcessMemberV1> = self
            .authority
            .process_group_members(self.pgid())
            .and_then(|members| {
                if members.len() > MAX_PROCESS_GROUP_MEMBERS {
                    Err(std::io::Error::other("process group member bound exceeded"))
                } else {
                    Ok(members)
                }
            })?
            .into_iter()
            .filter(|member| member.pgid == self.pgid())
            .map(|member| (member.identity.pid, member))
            .collect();
        let root = self.root_identity();
        let root_live = self
            .authority
            .immutable_start(root.pid)
            .is_ok_and(|current| current == *root);
        let mut registry = self
            .descendant_registry
            .lock()
            .map_err(|_| std::io::Error::other("owned descendant registry lock poisoned"))?;
        if root_live {
            registry.entry(root.pid).or_insert_with(|| ProcessMemberV1 {
                identity: root.clone(),
                parent_pid: 0,
                pgid: self.pgid(),
            });
        }
        if root_live && candidates.is_empty() {
            return Err(std::io::Error::other(
                "empty process-group census while the root identity is live",
            ));
        }

        // While any captured immutable generation is live, exact-PGID
        // membership is itself the admission anchor. This includes descendants
        // that double-forked or were reparented before their first census.
        let anchor_live = registry.values().any(|known| {
            self.authority
                .immutable_start(known.identity.pid)
                .is_ok_and(|current| current == known.identity)
        });
        if anchor_live {
            for (pid, member) in &candidates {
                registry.insert(*pid, member.clone());
            }
            return Ok(candidates.into_values().collect());
        }
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        // Once every immutable anchor is gone, a nonempty same-PGID census is
        // ambiguous: it may be the last helper or a recycled group. Refuse both
        // rather than turning the ambiguity into vacuous successful teardown.
        Err(std::io::Error::other(
            "same-PGID members remain after every process-tree anchor disappeared",
        ))
    }

    fn child_first(mut members: Vec<ProcessMemberV1>, root_pid: u32) -> Vec<ProcessMemberV1> {
        let parents: BTreeMap<_, _> = members
            .iter()
            .map(|member| (member.identity.pid, member.parent_pid))
            .collect();
        let depth = |pid: u32| {
            let mut depth = 0_usize;
            let mut cursor = pid;
            let mut seen = BTreeSet::new();
            while cursor != root_pid && seen.insert(cursor) {
                let Some(parent) = parents.get(&cursor).copied() else {
                    break;
                };
                depth = depth.saturating_add(1);
                cursor = parent;
            }
            depth
        };
        members.sort_by_key(|member| {
            (
                std::cmp::Reverse(depth(member.identity.pid)),
                member.identity.pid == root_pid,
                member.identity.pid,
            )
        });
        members
    }

    fn signal_term(&self, observations: &mut Vec<ProcessSignalObservationV1>) -> Option<String> {
        match self.members_with_live_root() {
            Ok(members) => {
                for member in Self::child_first(members, self.root_identity().pid) {
                    observations.push(self.observation_for(&member.identity, libc::SIGTERM));
                }
                None
            }
            Err(error) => Some(error.to_string()),
        }
    }

    fn resume_stopped_members(
        &self,
        stopped: BTreeMap<(u32, u64), ProcessStartIdentityV1>,
        observations: &mut Vec<ProcessSignalObservationV1>,
    ) {
        for identity in stopped.into_values() {
            observations.push(self.observation_for(&identity, libc::SIGCONT));
        }
    }

    fn close_and_kill(
        &self,
        observations: &mut Vec<ProcessSignalObservationV1>,
    ) -> Result<(), ProcessAuthorityErrorV1> {
        let mut stopped = BTreeMap::new();
        for _ in 0..CONTAINMENT_CLOSURE_LIMIT {
            let before = match self.members_with_live_root() {
                Ok(members) => members,
                Err(error) => {
                    self.resume_stopped_members(stopped, observations);
                    return Err(ProcessAuthorityErrorV1::CensusUnavailable {
                        detail: error.to_string(),
                    });
                }
            };
            for member in &before {
                observations.push(self.observation_for(&member.identity, libc::SIGSTOP));
                stopped.insert(
                    (member.identity.pid, member.identity.start_time_ticks),
                    member.identity.clone(),
                );
            }
            let after = match self.members_with_live_root() {
                Ok(members) => members,
                Err(error) => {
                    self.resume_stopped_members(stopped, observations);
                    return Err(ProcessAuthorityErrorV1::CensusUnavailable {
                        detail: error.to_string(),
                    });
                }
            };
            let before_set: BTreeSet<_> = before
                .iter()
                .map(|member| (member.identity.pid, member.identity.start_time_ticks))
                .collect();
            let after_set: BTreeSet<_> = after
                .iter()
                .map(|member| (member.identity.pid, member.identity.start_time_ticks))
                .collect();
            let all_contained = after.iter().all(|member| {
                match self.authority.process_is_stopped(&member.identity) {
                    Ok(contained) => contained,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                    Err(_) => false,
                }
            });
            if before_set == after_set && all_contained {
                for member in Self::child_first(after, self.root_identity().pid) {
                    observations.push(self.observation_for(&member.identity, libc::SIGKILL));
                }
                return Ok(());
            }
            if !all_contained {
                // SIGSTOP delivery is asynchronous. A running member cannot
                // authorize closure until the platform probe says stopped or
                // zombie, but every refusal path below resumes our volleys.
                std::thread::yield_now();
            }
        }
        self.resume_stopped_members(stopped, observations);
        Err(ProcessAuthorityErrorV1::ContainmentUnstable)
    }

    fn settle_dispatch(
        &self,
        observations: Vec<ProcessSignalObservationV1>,
        authority_error: Option<ProcessAuthorityErrorV1>,
    ) -> Result<ResourceActionResultV1, ProcessAuthorityErrorV1> {
        let syscall_failed = observations.iter().any(|observation| {
            observation.return_code == -1 && observation.errno != Some(libc::ESRCH)
        });
        let cause = authority_error.as_ref().and_then(|error| {
            let (failure_class, code) = match error {
                ProcessAuthorityErrorV1::ReapTimeout { .. } => {
                    (DiagnosticFailureClass::Timeout, "process.reap_timeout")
                }
                _ => (
                    DiagnosticFailureClass::AgentProcess,
                    "process.authority_failed",
                ),
            };
            DiagnosticCode::build(code, &DiagnosticRedactor::default())
                .ok()
                .map(|code| BoundedCauseV1 {
                    failure_class,
                    code,
                    deepest_cause: Some(error.to_string()),
                    cause_truncated: false,
                    evidence_overflow: false,
                    dependency_set: None,
                })
        });
        self.flight
            .journal_process_signals(observations)
            .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))?;
        let result = ResourceActionResultV1 {
            disposition: if authority_error.is_some() || syscall_failed {
                ResourceActionDispositionV1::Failed
            } else {
                ResourceActionDispositionV1::Complete
            },
            duration_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            recovery_owner: None,
            cause,
        };
        self.flight
            .settle(result)
            .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))
    }

    fn wait_direct_child_until(
        &self,
        child: &mut Option<Supervised>,
        deadline: std::time::Instant,
    ) -> std::io::Result<bool> {
        loop {
            match self.child_wait.try_wait(child.as_mut())? {
                DirectChildWaitStatusV1::Reaped => return Ok(true),
                DirectChildWaitStatusV1::Running if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::cmp::min(
                        BLOCKING_TERMINATE_POLL_INTERVAL,
                        deadline.saturating_duration_since(std::time::Instant::now()),
                    ));
                }
                DirectChildWaitStatusV1::Running => return Ok(false),
            }
        }
    }
    fn terminate_blocking_legacy_v2(
        &self,
        grace: std::time::Duration,
    ) -> Result<ResourceActionResultV1, ProcessAuthorityErrorV1> {
        match self.claim_action()? {
            ActionClaimV1::Finished | ActionClaimV1::Join => self.join_blocking(),
            ActionClaimV1::Drive => {
                let driving = DrivingActionGuardV1::new(self);
                // V2 keeps teardown effective even if its compatibility-only
                // in-memory evidence path refuses. V3 never dispatches after a
                // failed preparation.
                let preparation = self.prepare_dispatch();
                let mut child = match self.child.lock() {
                    Ok(mut child) => child.take(),
                    Err(poisoned) => poisoned.into_inner().take(),
                };
                let mut observations = vec![self.legacy_v2_group_observation(libc::SIGTERM)];
                let mut authority_error = None;
                let deadline = std::time::Instant::now()
                    .checked_add(grace)
                    .unwrap_or_else(std::time::Instant::now);
                let reaped = match self.wait_direct_child_until(&mut child, deadline) {
                    Ok(reaped) => reaped,
                    Err(error) => {
                        authority_error = Some(ProcessAuthorityErrorV1::IdentityUnavailable {
                            pid: self.root_identity().pid,
                            detail: error.to_string(),
                        });
                        false
                    }
                };
                if !reaped {
                    observations.push(self.legacy_v2_group_observation(libc::SIGKILL));
                    let reap_deadline = std::time::Instant::now()
                        .checked_add(self.reap_bound)
                        .unwrap_or_else(std::time::Instant::now);
                    match self.wait_direct_child_until(&mut child, reap_deadline) {
                        Ok(true) => {}
                        Ok(false) => {
                            authority_error.get_or_insert(ProcessAuthorityErrorV1::ReapTimeout {
                                pid: self.root_identity().pid,
                            });
                        }
                        Err(error) => {
                            authority_error.get_or_insert(
                                ProcessAuthorityErrorV1::IdentityUnavailable {
                                    pid: self.root_identity().pid,
                                    detail: error.to_string(),
                                },
                            );
                        }
                    }
                }
                // Historical Supervised::Drop unconditionally issued this final
                // group SIGKILL after either explicit terminate path.
                observations.push(self.legacy_v2_group_observation(libc::SIGKILL));
                let result = match preparation {
                    Ok(()) => self.settle_dispatch(observations, authority_error),
                    Err(error) => Err(error),
                };
                driving.finish(result)
            }
        }
    }

    fn terminate_blocking_inner(
        &self,
        grace: std::time::Duration,
    ) -> Result<ResourceActionResultV1, ProcessAuthorityErrorV1> {
        if self.mode == ProcessFlightModeV1::LegacyV2 {
            return self.terminate_blocking_legacy_v2(grace);
        }
        if let Err(error) = self.reject_recycled_pid_if_present() {
            return self.settle_identity_refusal(error);
        }
        match self.claim_action()? {
            ActionClaimV1::Finished | ActionClaimV1::Join => self.join_blocking(),
            ActionClaimV1::Drive => {
                let driving = DrivingActionGuardV1::new(self);
                if let Err(error) = self.prepare_dispatch() {
                    return driving.finish(Err(error));
                }
                let mut child = match self.child.lock() {
                    Ok(mut child) => child.take(),
                    Err(poisoned) => poisoned.into_inner().take(),
                };
                let mut observations = Vec::new();
                let mut authority_error = self.signal_term(&mut observations).map(|detail| {
                    ProcessAuthorityErrorV1::IdentityUnavailable {
                        pid: self.root_identity().pid,
                        detail,
                    }
                });
                let deadline = std::time::Instant::now()
                    .checked_add(grace)
                    .unwrap_or_else(std::time::Instant::now);
                let direct_child_reaped = match self.wait_direct_child_until(&mut child, deadline) {
                    Ok(reaped) => reaped,
                    Err(error) => {
                        authority_error.get_or_insert(
                            ProcessAuthorityErrorV1::IdentityUnavailable {
                                pid: self.root_identity().pid,
                                detail: error.to_string(),
                            },
                        );
                        false
                    }
                };
                if let Err(error) = self.close_and_kill(&mut observations) {
                    authority_error.get_or_insert(error);
                }
                if !direct_child_reaped {
                    let reap_deadline = std::time::Instant::now()
                        .checked_add(self.reap_bound)
                        .unwrap_or_else(std::time::Instant::now);
                    match self.wait_direct_child_until(&mut child, reap_deadline) {
                        Ok(true) => {}
                        Ok(false) => {
                            authority_error.get_or_insert(ProcessAuthorityErrorV1::ReapTimeout {
                                pid: self.root_identity().pid,
                            });
                        }
                        Err(error) => {
                            authority_error.get_or_insert(
                                ProcessAuthorityErrorV1::IdentityUnavailable {
                                    pid: self.root_identity().pid,
                                    detail: error.to_string(),
                                },
                            );
                        }
                    }
                }
                let result = self.settle_dispatch(observations, authority_error);
                driving.finish(result)
            }
        }
    }

    fn settle_identity_refusal(
        &self,
        error: ProcessAuthorityErrorV1,
    ) -> Result<ResourceActionResultV1, ProcessAuthorityErrorV1> {
        match self.claim_action()? {
            ActionClaimV1::Finished => Err(error),
            ActionClaimV1::Join => Err(error),
            ActionClaimV1::Drive => {
                let driving = DrivingActionGuardV1::new(self);
                if let Err(flight_error) = self.prepare_dispatch() {
                    return driving.finish(Err(flight_error));
                }
                let observation = ProcessSignalObservationV1 {
                    pid: self.root_identity().pid,
                    expected_start_time_ticks: self.root_identity().start_time_ticks,
                    signal: 0,
                    return_code: -1,
                    errno: Some(libc::ESTALE),
                };
                match self.settle_dispatch(vec![observation], Some(error.clone())) {
                    Ok(_) => driving.finish(Err(error)),
                    Err(flight_error) => driving.finish(Err(flight_error)),
                }
            }
        }
    }

    fn legacy_v2_drop(&self) -> Result<ResourceActionResultV1, ProcessAuthorityErrorV1> {
        let claim = match self.claim_action() {
            Ok(claim) => claim,
            Err(_) => {
                // A poisoned action lock cannot suppress the historical V2
                // backstop. Journal what remains possible, but signal exactly
                // once even when evidence settlement also refuses.
                let preparation = self.prepare_dispatch();
                let observation = self.legacy_v2_group_observation(libc::SIGKILL);
                let result = match preparation {
                    Ok(()) => self.settle_dispatch(vec![observation], None),
                    Err(error) => Err(error),
                };
                return self.finish_action(result);
            }
        };
        match claim {
            ActionClaimV1::Finished | ActionClaimV1::Join => self.join_blocking(),
            ActionClaimV1::Drive => {
                let driving = DrivingActionGuardV1::new(self);
                // Preserve the exact old Drop signal shape: one immediate group
                // SIGKILL, no SIGTERM, SIGSTOP, census, wait, or identity gate.
                let preparation = self.prepare_dispatch();
                let observation = self.legacy_v2_group_observation(libc::SIGKILL);
                let result = match preparation {
                    Ok(()) => self.settle_dispatch(vec![observation], None),
                    Err(error) => Err(error),
                };
                driving.finish(result)
            }
        }
    }

    fn protected_v3_drop_join_or_refuse(
        &self,
    ) -> Result<ResourceActionResultV1, ProcessAuthorityErrorV1> {
        match self.claim_action()? {
            ActionClaimV1::Finished | ActionClaimV1::Join => self.join_blocking(),
            ActionClaimV1::Drive => {
                let driving = DrivingActionGuardV1::new(self);
                let error = ProcessAuthorityErrorV1::Flight(
                    "V3 final drop had no journaled destructive intent; signals refused".into(),
                );
                let result = self.prepare_dispatch().and_then(|()| {
                    self.settle_dispatch(Vec::new(), Some(error.clone()))?;
                    Err(error.clone())
                });
                driving.finish(result)
            }
        }
    }
}

impl Drop for OwnedProcessStateV1 {
    fn drop(&mut self) {
        let result = match self.mode {
            ProcessFlightModeV1::LegacyV2 => self.legacy_v2_drop(),
            ProcessFlightModeV1::ProtectedV3 => self.protected_v3_drop_join_or_refuse(),
        };
        if let Err(error) = result {
            tracing::error!(
                error = %error,
                mode = ?self.mode,
                "process drop refused by retained resource flight"
            );
        }
    }
}

fn flight_io(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(format!("process resource flight: {error}"))
}

fn protective_binding_failure(
    flight: &RetainedResourceFlight,
    owner: &ResourceFlightOwnerV1,
    stage: ProcessBindingStageV1,
    pid: Option<u32>,
) -> Result<(), ProcessAuthorityErrorV1> {
    flight
        .journal_process_binding_failure(stage, pid)
        .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))?;
    flight
        .close_admission()
        .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))?;
    flight
        .journal_intent(ResourceActionIntentV1 {
            initiator: owner.clone(),
            capability_digest: Sha256HexV1::digest(b"process-binding-failure-v1"),
            cause: None,
        })
        .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))?;
    flight
        .begin_journaled_dispatch()
        .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))?;
    flight
        .journal_process_signals(Vec::new())
        .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))?;
    flight
        .settle(ResourceActionResultV1 {
            disposition: ResourceActionDispositionV1::Failed,
            duration_ms: 0,
            recovery_owner: None,
            cause: None,
        })
        .map(|_| ())
        .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))
}

impl OwnedProcessTreeV1 {
    pub fn spawn(
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
    ) -> std::io::Result<Self> {
        Self::spawn_with_stderr_redactor(prog, args, cwd, DiagnosticRedactor::default())
    }

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

    pub fn spawn_with_stderr_redactor_and_pinned_cwd(
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
        stderr_redactor: DiagnosticRedactor,
        pinned_cwd_fd: Option<std::os::fd::RawFd>,
        retain_pinned_cwd_fd_after_exec: bool,
    ) -> std::io::Result<Self> {
        Self::spawn_with_stderr_redactor_pinned_cwd_and_env_removals(
            prog,
            args,
            cwd,
            stderr_redactor,
            pinned_cwd_fd,
            retain_pinned_cwd_fd_after_exec,
            &[],
        )
    }

    pub fn spawn_with_stderr_redactor_pinned_cwd_and_env_removals(
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
        stderr_redactor: DiagnosticRedactor,
        pinned_cwd_fd: Option<std::os::fd::RawFd>,
        retain_pinned_cwd_fd_after_exec: bool,
        removed_environment: &[String],
    ) -> std::io::Result<Self> {
        Self::spawn_with_authority(
            prog,
            args,
            cwd,
            stderr_redactor,
            pinned_cwd_fd,
            retain_pinned_cwd_fd_after_exec,
            removed_environment,
            Arc::new(SystemProcessAuthorityV1),
            ProcessFlightProvisionV1::LegacyV2,
            Arc::new(SystemProcessSpawnV1),
        )
    }

    /// Spawn one protected V3 process generation using a caller-owned,
    /// file-journal-backed flight that exists before the child is created.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_with_durable_flight_v3(
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
        stderr_redactor: DiagnosticRedactor,
        pinned_cwd_fd: Option<std::os::fd::RawFd>,
        retain_pinned_cwd_fd_after_exec: bool,
        removed_environment: &[String],
        flight: DurableProcessFlightV3,
    ) -> std::io::Result<Self> {
        Self::spawn_with_authority(
            prog,
            args,
            cwd,
            stderr_redactor,
            pinned_cwd_fd,
            retain_pinned_cwd_fd_after_exec,
            removed_environment,
            Arc::new(SystemProcessAuthorityV1),
            ProcessFlightProvisionV1::DurableV3(flight),
            Arc::new(SystemProcessSpawnV1),
        )
    }

    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    fn spawn_with_authority(
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
        stderr_redactor: DiagnosticRedactor,
        pinned_cwd_fd: Option<std::os::fd::RawFd>,
        retain_pinned_cwd_fd_after_exec: bool,
        removed_environment: &[String],
        authority: Arc<dyn ProcessAuthorityPortV1>,
        provision: ProcessFlightProvisionV1,
        spawner: Arc<dyn ProcessSpawnPortV1>,
    ) -> std::io::Result<Self> {
        use ring::rand::{SecureRandom as _, SystemRandom};

        // The flight, registry, journal, and immutable generation material all
        // exist before Command::spawn. V2 mints one synthetic attempt-local
        // registry; V3 consumes the caller's one attempt registry and durable
        // file journal instead of manufacturing a second persistence scope.
        let (attempt_id, generation, flight_id, registry, journal, owner, result_publisher, mode) =
            match provision {
                ProcessFlightProvisionV1::LegacyV2 => {
                    let attempt_id = AttemptId::mint().map_err(flight_io)?;
                    let generation = format!("process-{}", attempt_id.as_str());
                    let owner = ResourceFlightOwnerV1::new(
                        NodeId::parse("process-spawn").map_err(flight_io)?,
                        generation.clone(),
                    )
                    .map_err(flight_io)?;
                    (
                        attempt_id.clone(),
                        generation,
                        ResourceFlightIdV1::mint().map_err(flight_io)?,
                        Arc::new(ResourceFlightRegistryV1::new(attempt_id)),
                        Arc::new(InMemoryResourceFlightJournal::new(512))
                            as Arc<dyn ResourceFlightJournal>,
                        owner,
                        Arc::new(NoopResourceFlightResultPublisher)
                            as Arc<dyn ResourceFlightResultPublisher>,
                        ProcessFlightModeV1::LegacyV2,
                    )
                }
                ProcessFlightProvisionV1::DurableV3(flight) => (
                    flight.attempt_id,
                    flight.generation,
                    flight.flight_id,
                    flight.registry,
                    flight.journal as Arc<dyn ResourceFlightJournal>,
                    flight.owner,
                    flight.result_publisher,
                    ProcessFlightModeV1::ProtectedV3,
                ),
            };
        let key = ResourceFlightKeyV1::AcpGeneration {
            generation: generation.clone(),
        };
        let reservation = registry
            .reserve(RetainedResourceFlightConfigV1 {
                flight_id,
                attempt_id,
                key,
                journal: Arc::clone(&journal),
                clock: Arc::new(SystemMonotonicClock::start()),
                result_publisher,
            })
            .map_err(flight_io)?;
        let flight = match reservation {
            ResourceFlightReservationV1::Created(flight) => flight,
            ResourceFlightReservationV1::Joined(_)
            | ResourceFlightReservationV1::Recovered { .. } => {
                return Err(flight_io(
                    "new process generation did not create a unique flight",
                ));
            }
        };
        match mode {
            ProcessFlightModeV1::LegacyV2 => {
                flight
                    .attach_owner_legacy_v2(owner.clone())
                    .map_err(flight_io)?;
            }
            ProcessFlightModeV1::ProtectedV3 => {
                flight.attach_owner(owner.clone()).map_err(flight_io)?;
                flight.reserve_process_lifecycle().map_err(flight_io)?;
            }
        }

        let mut spawn_nonce = [0_u8; 32];
        SystemRandom::new()
            .fill(&mut spawn_nonce)
            .map_err(|_| flight_io("spawn nonce unavailable"))?;
        let supervised = match spawner.spawn_owned(
            prog,
            args,
            cwd,
            stderr_redactor,
            pinned_cwd_fd,
            retain_pinned_cwd_fd_after_exec,
            removed_environment,
            mode == ProcessFlightModeV1::LegacyV2,
        ) {
            Ok(supervised) => supervised,
            Err(error) => {
                if let Err(protection) =
                    protective_binding_failure(&flight, &owner, ProcessBindingStageV1::Spawn, None)
                {
                    return Err(flight_io(protection));
                }
                return Err(error);
            }
        };
        let pid = supervised.pid;
        let immutable_start = match authority.immutable_start(pid) {
            Ok(identity) if identity.pid == pid => identity,
            Ok(_) => {
                protective_binding_failure(
                    &flight,
                    &owner,
                    ProcessBindingStageV1::ImmutableStart,
                    Some(pid),
                )
                .map_err(flight_io)?;
                return Err(flight_io("immutable process identity pid mismatch"));
            }
            Err(error) => {
                protective_binding_failure(
                    &flight,
                    &owner,
                    ProcessBindingStageV1::ImmutableStart,
                    Some(pid),
                )
                .map_err(flight_io)?;
                return Err(error);
            }
        };
        let immutable_start_for_registry = immutable_start.clone();
        let captured =
            OwnedProcessTreeV1::capture(generation, spawn_nonce, pid, Some(pid), immutable_start)
                .map_err(flight_io)?;
        if let Err(error) = captured.journal_capture(&flight) {
            protective_binding_failure(
                &flight,
                &owner,
                ProcessBindingStageV1::JournalBinding,
                Some(pid),
            )
            .map_err(flight_io)?;
            return Err(flight_io(error));
        }
        let identity = captured.identity().clone();
        Ok(OwnedProcessTreeV1::from_bound(
            identity.clone(),
            Arc::new(OwnedProcessStateV1 {
                identity,
                child_wait: Arc::new(SystemDirectChildWaitV1),
                child: Mutex::new(Some(supervised)),
                authority,
                descendant_registry: Mutex::new(BTreeMap::from([(
                    pid,
                    ProcessMemberV1 {
                        identity: immutable_start_for_registry.clone(),
                        parent_pid: 0,
                        pgid: pid,
                    },
                )])),
                flight,
                _registry: registry,
                journal,
                owner,
                reap_bound: BLOCKING_TERMINATE_REAP_BOUND,
                mode,
                started: std::time::Instant::now(),
                action: Mutex::new(OwnedActionStateV1::Idle),
                action_settled: Condvar::new(),
            }),
        ))
    }

    fn live(&self) -> Result<&Arc<OwnedProcessStateV1>, ProcessAuthorityErrorV1> {
        self.process
            .as_ref()
            .ok_or(ProcessAuthorityErrorV1::ProcessUnavailable)
    }

    pub fn pid(&self) -> u32 {
        match self.identity() {
            ResourceIdentityV1::AcpProcess { pid, .. } => *pid,
            _ => 0,
        }
    }

    pub fn stderr_ring(&self) -> ProcessStderrRing {
        self.live()
            .ok()
            .and_then(|state| state.child.lock().ok())
            .and_then(|child| child.as_ref().map(Supervised::stderr_ring))
            .unwrap_or_default()
    }

    pub fn apply_stderr_redactor(&self, redactor: DiagnosticRedactor) {
        if let Ok(state) = self.live() {
            if let Ok(child) = state.child.lock() {
                if let Some(child) = child.as_ref() {
                    child.apply_stderr_redactor(redactor);
                }
            }
        }
    }

    pub fn child_mut(&mut self) -> OwnedChildGuardV1<'_> {
        OwnedChildGuardV1 {
            guard: self
                .live()
                .expect("capture-only process has no child")
                .child
                .lock()
                .expect("owned process child lock"),
        }
    }

    pub fn attach_owner(
        &self,
        owner: ResourceFlightOwnerV1,
    ) -> Result<bool, ProcessAuthorityErrorV1> {
        let state = self.live()?;
        match state.mode {
            ProcessFlightModeV1::LegacyV2 => state.flight.attach_owner_legacy_v2(owner),
            ProcessFlightModeV1::ProtectedV3 => state.flight.attach_owner(owner),
        }
        .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))
    }

    pub fn detach_owner(
        &self,
        owner: &ResourceFlightOwnerV1,
    ) -> Result<bool, ProcessAuthorityErrorV1> {
        let state = self.live()?;
        match state.mode {
            ProcessFlightModeV1::LegacyV2 => state.flight.detach_owner_legacy_v2(owner),
            ProcessFlightModeV1::ProtectedV3 => state.flight.detach_owner(owner),
        }
        .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))
    }

    pub async fn terminate(
        self,
        grace: std::time::Duration,
    ) -> Result<ResourceActionResultV1, ProcessAuthorityErrorV1> {
        let state = Arc::clone(self.live()?);
        // A dropped caller cannot abandon the process flight in Driving:
        // spawn_blocking workers run to completion once started, while every
        // concurrent caller joins the same state machine.
        tokio::task::spawn_blocking(move || state.terminate_blocking_inner(grace))
            .await
            .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))?
    }

    #[doc(hidden)]
    pub fn terminate_blocking(
        self,
        grace: std::time::Duration,
    ) -> Result<ResourceActionResultV1, ProcessAuthorityErrorV1> {
        self.live()?.terminate_blocking_inner(grace)
    }

    pub fn join_blocking(&self) -> Result<ResourceActionResultV1, ProcessAuthorityErrorV1> {
        self.live()?.join_blocking()
    }

    #[must_use]
    pub fn is_legacy_v2(&self) -> bool {
        self.live()
            .is_ok_and(|state| state.mode == ProcessFlightModeV1::LegacyV2)
    }

    /// Ordinary adapter Drop joins an already-running protected flight or
    /// durably refuses an unarmed one. Legacy V2 returns without initiating
    /// work so field destruction retains its historical single-SIGKILL path.
    #[doc(hidden)]
    pub fn final_drop_join_or_refuse(
        &self,
    ) -> Result<Option<ResourceActionResultV1>, ProcessAuthorityErrorV1> {
        let state = self.live()?;
        (state.mode == ProcessFlightModeV1::ProtectedV3)
            .then(|| state.protected_v3_drop_join_or_refuse())
            .transpose()
    }

    pub fn flight_state(&self) -> Result<ResourceFlightStateV1, ProcessAuthorityErrorV1> {
        self.live()?
            .flight
            .state()
            .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))
    }

    #[doc(hidden)]
    pub fn journal_records(
        &self,
    ) -> Result<
        Vec<crate::retained_resource_flight::ResourceFlightJournalRecordV1>,
        ProcessAuthorityErrorV1,
    > {
        let state = self.live()?;
        state
            .journal
            .records(state.flight.flight_id())
            .map_err(|error| ProcessAuthorityErrorV1::Flight(error.to_string()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd as _;
    use std::time::Duration;

    // returns the `stat`/`state` column for a pid, or None if the pid is gone/reaped.
    #[cfg(target_os = "linux")]
    fn proc_state(pid: u32) -> Option<String> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.rsplit_once(") ")?
            .1
            .split_whitespace()
            .next()
            .map(str::to_owned)
    }

    #[cfg(not(target_os = "linux"))]
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
    async fn bounded_spawn_removes_named_variables_from_child_environment() {
        use tokio::io::AsyncReadExt as _;

        const VARIABLE: &str = "A2A_BRIDGE_TEST_SECRET_SOURCE_ENV_R2F1A";
        const SECRET: &str = "must-not-reach-adapter-child";
        std::env::set_var(VARIABLE, SECRET);
        let command = format!("printf %s \"${{{VARIABLE}-}}\"");
        let mut child = Supervised::spawn_with_stderr_redactor_pinned_cwd_and_env_removals(
            "/bin/sh",
            &["-c", &command],
            None,
            DiagnosticRedactor::new([SECRET]),
            None,
            false,
            &[VARIABLE.to_string()],
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
        std::env::remove_var(VARIABLE);

        assert!(stdout.is_empty(), "source secret reached adapter child");
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
        assert_eq!(snapshot.byte_count(), 12);
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
    async fn drop_group_kills_descendants_host_signal_semantics() {
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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn protected_v3_child_grandchild_stop_then_child_first_kill_has_no_live_leaks_host_signal_semantics(
    ) {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let root = tempfile::tempdir().unwrap();
        let attempt_id = AttemptId::mint().unwrap();
        let generation = format!("host-containment-{}", attempt_id.as_str());
        let journal = Arc::new(FileResourceFlightJournal::open(root.path(), 512).unwrap());
        let owner = ResourceFlightOwnerV1::new(
            NodeId::parse("host-containment").unwrap(),
            generation.clone(),
        )
        .unwrap();
        let durable = DurableProcessFlightV3::new(
            attempt_id.clone(),
            generation,
            Arc::new(ResourceFlightRegistryV1::new(attempt_id)),
            journal.clone(),
            owner,
        )
        .unwrap();
        let flight_id = durable.flight_id().clone();
        let script = "trap '' TERM; /bin/sh -c 'trap \"\" TERM; \
                      printf \"READY_CHILD %s\\n\" $$; exec sleep 300' & descendant=$!; \
                      printf 'READY_ROOT %s %s\\n' $$ $descendant; wait";
        let mut process = OwnedProcessTreeV1::spawn_with_durable_flight_v3(
            "/bin/sh",
            &["-c", script],
            None,
            DiagnosticRedactor::default(),
            None,
            false,
            &[],
            durable,
        )
        .unwrap();
        let root_pid = process.pid();
        let stdout = process.child_mut().stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();
        let ready = tokio::time::timeout(Duration::from_secs(2), async {
            let mut messages = Vec::new();
            for _ in 0..2 {
                messages.push(
                    lines
                        .next_line()
                        .await
                        .unwrap()
                        .expect("both processes must publish readiness"),
                );
            }
            messages
        })
        .await
        .expect("child/grandchild readiness handshake timed out");
        let mut reported_root = None;
        let mut reported_descendant = None;
        let mut child_ready = None;
        for message in ready {
            let fields: Vec<_> = message.split_whitespace().collect();
            match fields.as_slice() {
                ["READY_ROOT", root, descendant] => {
                    reported_root = Some(root.parse::<u32>().unwrap());
                    reported_descendant = Some(descendant.parse::<u32>().unwrap());
                }
                ["READY_CHILD", child] => child_ready = Some(child.parse::<u32>().unwrap()),
                other => panic!("unexpected readiness message: {other:?}"),
            }
        }
        let descendant_pid = reported_descendant.expect("root descendant pid");
        assert_eq!(reported_root, Some(root_pid));
        assert_eq!(child_ready, Some(descendant_pid));
        assert!(proc_state(root_pid).is_some());
        assert!(proc_state(descendant_pid).is_some());

        let result = process.terminate(Duration::ZERO).await.unwrap();
        assert_eq!(
            result.disposition,
            ResourceActionDispositionV1::Complete,
            "the protected flight must close and reap the exact generation"
        );

        let records = journal.records(&flight_id).unwrap();
        let observations = records
            .iter()
            .find_map(|row| match &row.event {
                crate::retained_resource_flight::ResourceFlightJournalEventV1::ProcessSignalsObserved {
                    observations,
                } => Some(observations),
                _ => None,
            })
            .expect("flight signal observations");
        let stop_root = observations
            .iter()
            .position(|entry| entry.pid == root_pid && entry.signal == libc::SIGSTOP)
            .expect("root SIGSTOP");
        let stop_descendant = observations
            .iter()
            .position(|entry| entry.pid == descendant_pid && entry.signal == libc::SIGSTOP)
            .expect("descendant SIGSTOP");
        let kill_descendant = observations
            .iter()
            .position(|entry| entry.pid == descendant_pid && entry.signal == libc::SIGKILL)
            .expect("descendant SIGKILL after verified stop");
        let kill_root = observations
            .iter()
            .position(|entry| entry.pid == root_pid && entry.signal == libc::SIGKILL)
            .expect("root SIGKILL after verified stop");
        assert!(stop_root < kill_root);
        assert!(stop_descendant < kill_descendant);
        assert!(kill_descendant < kill_root, "SIGKILL must be child-first");

        async fn wait_until_not_running(pid: u32) {
            tokio::time::timeout(Duration::from_secs(2), async move {
                loop {
                    match proc_state(pid) {
                        None => break,
                        Some(state) if state.starts_with('Z') => break,
                        Some(_) => tokio::task::yield_now().await,
                    }
                }
            })
            .await
            .unwrap_or_else(|_| panic!("pid {pid} remained live after protected settlement"));
        }
        wait_until_not_running(root_pid).await;
        wait_until_not_running(descendant_pid).await;
    }

    #[tokio::test]
    async fn terminate_reaps_child_no_zombie_host_signal_semantics() {
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

    #[test]
    fn blocking_terminate_reaps_after_source_runtime_shutdown_host_signal_semantics() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let child = runtime.block_on(async {
            Supervised::spawn(
                "/bin/sh",
                &["-c", "trap '' TERM; while :; do sleep 1; done"],
                None,
            )
            .unwrap()
        });
        let pid = child.pid();
        drop(runtime);

        child.terminate_blocking(Duration::from_millis(50));

        assert_eq!(
            proc_state(pid),
            None,
            "runtime-independent termination must reap the exact child"
        );
    }

    #[tokio::test]
    async fn term_ignoring_child_with_descendant_is_group_killed_host_signal_semantics() {
        fn terminated(state: Option<String>) -> bool {
            state.is_none_or(|state| state.starts_with('Z'))
        }

        async fn wait_until_terminated(pid: u32, label: &str) {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if terminated(proc_state(pid)) {
                        return;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("{label} {pid} survived group kill"));
        }

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
        let desc = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(contents) = std::fs::read_to_string(format!("/tmp/a2a_desc_pid.{pid}")) {
                    if let Ok(descendant) = contents.trim().parse::<u32>() {
                        return descendant;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("descendant readiness timed out");
        child.terminate(Duration::from_millis(300)).await; // grace then SIGKILL group
        wait_until_terminated(pid, "leader").await;
        wait_until_terminated(desc, "descendant").await;

        // Red-first controls: strict pid absence would reject a killed child
        // held as a zombie, while a genuinely running child remains a leak.
        let zombie = Some("Z (sleep)".to_string());
        assert!(zombie.is_some(), "strict absence would reject this zombie");
        assert!(terminated(zombie), "zombies are terminated descendants");
        assert!(
            !terminated(Some("S (sleep)".to_string())),
            "a live descendant must remain a failure"
        );
        let _ = std::fs::remove_file(format!("/tmp/a2a_desc_pid.{pid}"));
    }

    #[tokio::test]
    async fn term_ignoring_loop_forces_group_sigkill_host_signal_semantics() {
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

    struct SignalGateV1 {
        pid: u32,
        signal: i32,
        entered: std::sync::Barrier,
        release: std::sync::Barrier,
    }

    impl SignalGateV1 {
        fn new(pid: u32, signal: i32) -> Self {
            Self {
                pid,
                signal,
                entered: std::sync::Barrier::new(2),
                release: std::sync::Barrier::new(2),
            }
        }
    }

    struct ProbeGateV1 {
        entered: std::sync::Barrier,
        release: std::sync::Barrier,
    }

    impl ProbeGateV1 {
        fn new() -> Self {
            Self {
                entered: std::sync::Barrier::new(2),
                release: std::sync::Barrier::new(2),
            }
        }
    }

    #[derive(Default)]
    struct FakeProcessAuthority {
        identities: Mutex<BTreeMap<u32, ProcessStartIdentityV1>>,
        censuses: Mutex<VecDeque<Vec<ProcessMemberV1>>>,
        signals: Mutex<Vec<(u32, i32)>>,
        group_signals: Mutex<Vec<(u32, i32)>>,
        fail_signal: Mutex<Option<i32>>,
        stop_probe_delays: Mutex<BTreeMap<u32, usize>>,
        zombie_members: Mutex<BTreeSet<u32>>,
        remove_on_signal: Mutex<BTreeSet<(u32, i32)>>,
        signal_gate: Mutex<Option<Arc<SignalGateV1>>>,
        not_found_gate: Mutex<Option<Arc<ProbeGateV1>>>,
    }

    impl FakeProcessAuthority {
        fn with_censuses(censuses: Vec<Vec<ProcessMemberV1>>) -> Self {
            Self {
                censuses: Mutex::new(censuses.into()),
                ..Self::default()
            }
        }

        fn install(&self, identity: ProcessStartIdentityV1) {
            self.identities
                .lock()
                .unwrap()
                .insert(identity.pid, identity);
        }

        fn signals(&self) -> Vec<(u32, i32)> {
            self.signals.lock().unwrap().clone()
        }

        fn group_signals(&self) -> Vec<(u32, i32)> {
            self.group_signals.lock().unwrap().clone()
        }

        fn delay_stop_probe(&self, pid: u32, failed_probes: usize) {
            self.stop_probe_delays
                .lock()
                .unwrap()
                .insert(pid, failed_probes);
        }

        fn mark_zombie(&self, pid: u32) {
            self.zombie_members.lock().unwrap().insert(pid);
        }

        fn remove_identity_on_signal(&self, pid: u32, signal: i32) {
            self.remove_on_signal.lock().unwrap().insert((pid, signal));
        }

        fn remove(&self, pid: u32) {
            self.identities.lock().unwrap().remove(&pid);
        }

        fn gate_signal(&self, pid: u32, signal: i32) -> Arc<SignalGateV1> {
            let gate = Arc::new(SignalGateV1::new(pid, signal));
            *self.signal_gate.lock().unwrap() = Some(Arc::clone(&gate));
            gate
        }

        fn gate_next_not_found(&self) -> Arc<ProbeGateV1> {
            let gate = Arc::new(ProbeGateV1::new());
            *self.not_found_gate.lock().unwrap() = Some(Arc::clone(&gate));
            gate
        }
    }

    impl ProcessAuthorityPortV1 for FakeProcessAuthority {
        fn immutable_start(&self, pid: u32) -> std::io::Result<ProcessStartIdentityV1> {
            if let Some(identity) = self.identities.lock().unwrap().get(&pid).cloned() {
                return Ok(identity);
            }
            if let Some(gate) = self.not_found_gate.lock().unwrap().take() {
                gate.entered.wait();
                gate.release.wait();
            }
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        }

        fn process_group_members(&self, _: u32) -> std::io::Result<Vec<ProcessMemberV1>> {
            let mut censuses = self.censuses.lock().unwrap();
            let value = if censuses.len() > 1 {
                censuses.pop_front().unwrap()
            } else {
                censuses.front().cloned().unwrap_or_default()
            };
            Ok(value)
        }

        fn process_is_stopped(&self, expected: &ProcessStartIdentityV1) -> std::io::Result<bool> {
            if self
                .identities
                .lock()
                .unwrap()
                .get(&expected.pid)
                .is_none_or(|current| current != expected)
            {
                return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
            }
            if self.zombie_members.lock().unwrap().contains(&expected.pid) {
                return Ok(true);
            }
            if !self
                .signals
                .lock()
                .unwrap()
                .contains(&(expected.pid, libc::SIGSTOP))
            {
                return Ok(false);
            }
            let mut delays = self.stop_probe_delays.lock().unwrap();
            let remaining = delays.entry(expected.pid).or_default();
            if *remaining == 0 {
                Ok(true)
            } else {
                *remaining -= 1;
                Ok(false)
            }
        }

        fn signal_process(&self, pid: u32, signal: i32) -> ProcessSyscallOutcomeV1 {
            self.signals.lock().unwrap().push((pid, signal));
            if self
                .remove_on_signal
                .lock()
                .unwrap()
                .contains(&(pid, signal))
            {
                self.identities.lock().unwrap().remove(&pid);
            }
            let gate = self.signal_gate.lock().unwrap().clone();
            if let Some(gate) = gate.filter(|gate| gate.pid == pid && gate.signal == signal) {
                gate.entered.wait();
                gate.release.wait();
            }
            if *self.fail_signal.lock().unwrap() == Some(signal) {
                ProcessSyscallOutcomeV1 {
                    return_code: -1,
                    errno: Some(libc::EPERM),
                }
            } else {
                ProcessSyscallOutcomeV1 {
                    return_code: 0,
                    errno: None,
                }
            }
        }

        fn signal_process_group(&self, pgid: u32, signal: i32) -> ProcessSyscallOutcomeV1 {
            self.group_signals.lock().unwrap().push((pgid, signal));
            if *self.fail_signal.lock().unwrap() == Some(signal) {
                ProcessSyscallOutcomeV1 {
                    return_code: -1,
                    errno: Some(libc::EPERM),
                }
            } else {
                ProcessSyscallOutcomeV1 {
                    return_code: 0,
                    errno: None,
                }
            }
        }
    }

    fn process_identity(pid: u32, start: u64) -> ProcessStartIdentityV1 {
        ProcessStartIdentityV1 {
            pid,
            start_time_ticks: start,
            executable_sha256: None,
        }
    }

    fn member(pid: u32, start: u64, parent_pid: u32) -> ProcessMemberV1 {
        ProcessMemberV1 {
            identity: process_identity(pid, start),
            parent_pid,
            pgid: 42,
        }
    }

    #[derive(Default)]
    struct ReapedChildWaitV1;

    impl DirectChildWaitPortV1 for ReapedChildWaitV1 {
        fn try_wait(&self, _: Option<&mut Supervised>) -> std::io::Result<DirectChildWaitStatusV1> {
            Ok(DirectChildWaitStatusV1::Reaped)
        }
    }

    #[derive(Default)]
    struct NeverReapedChildWaitV1;

    impl DirectChildWaitPortV1 for NeverReapedChildWaitV1 {
        fn try_wait(&self, _: Option<&mut Supervised>) -> std::io::Result<DirectChildWaitStatusV1> {
            Ok(DirectChildWaitStatusV1::Running)
        }
    }

    #[derive(Default)]
    struct RecordingProcessPublisher(
        Mutex<Vec<crate::retained_resource_flight::NodeCleanupAggregationV1>>,
    );

    impl ResourceFlightResultPublisher for RecordingProcessPublisher {
        fn publish(&self, aggregation: crate::retained_resource_flight::NodeCleanupAggregationV1) {
            self.0.lock().unwrap().push(aggregation);
        }
    }

    struct RecoveryWinsProcessJournal {
        inner: InMemoryResourceFlightJournal,
        driver_at_terminal: std::sync::Barrier,
        recovery_done: std::sync::Barrier,
    }

    impl RecoveryWinsProcessJournal {
        fn new() -> Self {
            Self {
                inner: InMemoryResourceFlightJournal::new(512),
                driver_at_terminal: std::sync::Barrier::new(2),
                recovery_done: std::sync::Barrier::new(2),
            }
        }
    }

    impl ResourceFlightJournal for RecoveryWinsProcessJournal {
        fn reserve_flight(
            &self,
            reservation: &crate::retained_resource_flight::ResourceFlightReservationRecordV1,
        ) -> Result<
            crate::retained_resource_flight::ResourceFlightReservationOutcomeV1,
            crate::retained_resource_flight::ResourceFlightJournalError,
        > {
            self.inner.reserve_flight(reservation)
        }

        fn reservations(
            &self,
        ) -> Result<
            Vec<crate::retained_resource_flight::ResourceFlightReservationRecordV1>,
            crate::retained_resource_flight::ResourceFlightJournalError,
        > {
            self.inner.reservations()
        }

        fn rollback_empty_reservation(
            &self,
            reservation: &crate::retained_resource_flight::ResourceFlightReservationRecordV1,
        ) -> Result<bool, crate::retained_resource_flight::ResourceFlightJournalError> {
            self.inner.rollback_empty_reservation(reservation)
        }

        fn append(
            &self,
            id: &ResourceFlightIdV1,
            record: &crate::retained_resource_flight::ResourceFlightJournalRecordV1,
        ) -> Result<(), crate::retained_resource_flight::ResourceFlightJournalError> {
            self.inner.append(id, record)
        }

        fn append_terminal(
            &self,
            id: &ResourceFlightIdV1,
            result: &ResourceActionResultV1,
        ) -> Result<
            crate::retained_resource_flight::ResourceFlightTerminalAppendOutcomeV1,
            crate::retained_resource_flight::ResourceFlightJournalError,
        > {
            if result.disposition == ResourceActionDispositionV1::Complete {
                self.driver_at_terminal.wait();
                self.recovery_done.wait();
            }
            self.inner.append_terminal(id, result)
        }

        fn records(
            &self,
            id: &ResourceFlightIdV1,
        ) -> Result<
            Vec<crate::retained_resource_flight::ResourceFlightJournalRecordV1>,
            crate::retained_resource_flight::ResourceFlightJournalError,
        > {
            self.inner.records(id)
        }
    }

    #[derive(Default)]
    struct CountingSpawnPortV1 {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl ProcessSpawnPortV1 for CountingSpawnPortV1 {
        fn spawn_owned(
            &self,
            _: &str,
            _: &[&str],
            _: Option<&std::path::Path>,
            _: DiagnosticRedactor,
            _: Option<std::os::fd::RawFd>,
            _: bool,
            _: &[String],
            _: bool,
        ) -> std::io::Result<Supervised> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(std::io::Error::other("injected spawn"))
        }
    }

    fn true_program_path() -> &'static str {
        #[cfg(target_os = "macos")]
        {
            "/usr/bin/true"
        }
        #[cfg(not(target_os = "macos"))]
        {
            "/bin/true"
        }
    }

    fn fake_owned(
        authority: Arc<FakeProcessAuthority>,
        mode: ProcessFlightModeV1,
    ) -> (OwnedProcessTreeV1, Arc<InMemoryResourceFlightJournal>) {
        fake_owned_with_wait(
            authority,
            mode,
            Arc::new(ReapedChildWaitV1),
            BLOCKING_TERMINATE_REAP_BOUND,
        )
    }

    fn fake_owned_with_wait(
        authority: Arc<FakeProcessAuthority>,
        mode: ProcessFlightModeV1,
        child_wait: Arc<dyn DirectChildWaitPortV1>,
        reap_bound: Duration,
    ) -> (OwnedProcessTreeV1, Arc<InMemoryResourceFlightJournal>) {
        let journal = Arc::new(InMemoryResourceFlightJournal::new(512));
        let journal_port: Arc<dyn ResourceFlightJournal> = journal.clone();
        let process = fake_owned_with_components(
            authority,
            mode,
            child_wait,
            reap_bound,
            journal_port,
            Arc::new(NoopResourceFlightResultPublisher),
        );
        (process, journal)
    }

    fn fake_owned_with_components(
        authority: Arc<FakeProcessAuthority>,
        mode: ProcessFlightModeV1,
        child_wait: Arc<dyn DirectChildWaitPortV1>,
        reap_bound: Duration,
        journal: Arc<dyn ResourceFlightJournal>,
        result_publisher: Arc<dyn ResourceFlightResultPublisher>,
    ) -> OwnedProcessTreeV1 {
        let attempt_id = AttemptId::mint().unwrap();
        let generation = format!("fake-{}", attempt_id.as_str());
        let registry = Arc::new(ResourceFlightRegistryV1::new(attempt_id.clone()));
        let reservation = registry
            .reserve(RetainedResourceFlightConfigV1 {
                flight_id: ResourceFlightIdV1::mint().unwrap(),
                attempt_id,
                key: ResourceFlightKeyV1::AcpGeneration {
                    generation: generation.clone(),
                },
                journal: journal.clone(),
                clock: Arc::new(SystemMonotonicClock::start()),
                result_publisher,
            })
            .unwrap();
        let flight = Arc::clone(reservation.flight().unwrap());
        let owner =
            ResourceFlightOwnerV1::new(NodeId::parse("process-spawn").unwrap(), generation.clone())
                .unwrap();
        flight.attach_owner(owner.clone()).unwrap();
        if mode == ProcessFlightModeV1::ProtectedV3 {
            flight.reserve_process_lifecycle().unwrap();
        }
        let identity = ResourceIdentityV1::AcpProcess {
            generation,
            spawn_nonce_sha256: Sha256HexV1::digest(b"fake-spawn"),
            pid: 42,
            pgid: Some(42),
            immutable_start: process_identity(42, 7),
        };
        flight.journal_process_tree(identity.clone()).unwrap();
        let authority_port: Arc<dyn ProcessAuthorityPortV1> = authority;
        OwnedProcessTreeV1::from_bound(
            identity.clone(),
            Arc::new(OwnedProcessStateV1 {
                identity,
                child_wait,
                child: Mutex::new(None),
                authority: authority_port,
                descendant_registry: Mutex::new(BTreeMap::from([(
                    42,
                    ProcessMemberV1 {
                        identity: process_identity(42, 7),
                        parent_pid: 0,
                        pgid: 42,
                    },
                )])),
                flight,
                _registry: registry,
                journal,
                owner,
                reap_bound,
                mode,
                started: std::time::Instant::now(),
                action: Mutex::new(OwnedActionStateV1::Idle),
                action_settled: Condvar::new(),
            }),
        )
    }

    #[test]
    fn unrelated_process_with_recycled_pid_survives_every_flight_action() {
        let fake = Arc::new(FakeProcessAuthority::default());
        fake.install(process_identity(42, 700));
        let (process, _) = fake_owned(fake.clone(), ProcessFlightModeV1::ProtectedV3);

        assert!(matches!(
            process.clone().terminate_blocking(Duration::from_millis(1)),
            Err(ProcessAuthorityErrorV1::IdentityChanged { pid: 42 })
        ));
        assert!(matches!(
            process.join_blocking(),
            Err(ProcessAuthorityErrorV1::IdentityChanged { pid: 42 })
        ));
        drop(process);
        assert!(fake.signals().is_empty(), "recycled pid was never signaled");

        // A generation that already settled must not authorize a later process
        // which recycles the same numeric pid/pgid either.
        let root = member(42, 7, 0);
        let settled_fake = Arc::new(FakeProcessAuthority::with_censuses(vec![
            vec![root.clone()],
            vec![root.clone()],
            vec![root],
        ]));
        settled_fake.install(process_identity(42, 7));
        let (settled, _) = fake_owned(settled_fake.clone(), ProcessFlightModeV1::ProtectedV3);
        settled
            .clone()
            .terminate_blocking(Duration::from_millis(1))
            .unwrap();
        let signal_count = settled_fake.signals().len();
        settled_fake.install(process_identity(42, 700));
        assert!(matches!(
            settled.clone().terminate_blocking(Duration::from_millis(1)),
            Err(ProcessAuthorityErrorV1::IdentityChanged { pid: 42 })
        ));
        assert!(matches!(
            settled.join_blocking(),
            Err(ProcessAuthorityErrorV1::IdentityChanged { pid: 42 })
        ));
        drop(settled);
        assert_eq!(settled_fake.signals().len(), signal_count);
    }

    #[test]
    fn two_nodes_one_generation_signal_once_and_share_result() {
        let root = member(42, 7, 0);
        let fake = Arc::new(FakeProcessAuthority::with_censuses(vec![
            vec![root.clone()],
            vec![root.clone()],
            vec![root],
        ]));
        fake.install(process_identity(42, 7));
        let (process, _) = fake_owned(fake.clone(), ProcessFlightModeV1::ProtectedV3);
        let first =
            ResourceFlightOwnerV1::new(NodeId::parse("node-a").unwrap(), "session-a").unwrap();
        let second =
            ResourceFlightOwnerV1::new(NodeId::parse("node-b").unwrap(), "session-b").unwrap();
        process.attach_owner(first).unwrap();
        process.attach_owner(second).unwrap();

        let left = process.clone();
        let right = process.clone();
        let left =
            std::thread::spawn(move || left.terminate_blocking(Duration::from_millis(1)).unwrap());
        let right =
            std::thread::spawn(move || right.terminate_blocking(Duration::from_millis(1)).unwrap());
        assert_eq!(left.join().unwrap(), right.join().unwrap());
        let signals = fake.signals();
        assert_eq!(
            signals
                .iter()
                .filter(|(_, signal)| *signal == libc::SIGTERM)
                .count(),
            1
        );
        assert_eq!(
            signals
                .iter()
                .filter(|(_, signal)| *signal == libc::SIGKILL)
                .count(),
            1
        );
    }

    #[test]
    fn ordinary_release_detaches_one_owner_without_signaling() {
        let fake = Arc::new(FakeProcessAuthority::default());
        fake.install(process_identity(42, 7));
        let (process, _) = fake_owned(fake.clone(), ProcessFlightModeV1::ProtectedV3);
        let owner =
            ResourceFlightOwnerV1::new(NodeId::parse("node-a").unwrap(), "session-a").unwrap();
        process.attach_owner(owner.clone()).unwrap();
        assert!(process.detach_owner(&owner).unwrap());
        assert!(fake.signals().is_empty());
        assert!(matches!(
            process.flight_state().unwrap(),
            ResourceFlightStateV1::Open {}
        ));
        assert!(process
            .journal_records()
            .unwrap()
            .iter()
            .any(|row| matches!(
                &row.event,
                crate::retained_resource_flight::ResourceFlightJournalEventV1::OwnerDetached {
                    owner: observed
                } if observed == &owner
            )));
    }

    #[test]
    fn shared_acp_escalation_lists_every_active_owner() {
        let root = member(42, 7, 0);
        let fake = Arc::new(FakeProcessAuthority::with_censuses(vec![
            vec![root.clone()],
            vec![root.clone()],
            vec![root],
        ]));
        fake.install(process_identity(42, 7));
        let (process, _) = fake_owned(fake, ProcessFlightModeV1::ProtectedV3);
        let first =
            ResourceFlightOwnerV1::new(NodeId::parse("node-a").unwrap(), "session-a").unwrap();
        let second =
            ResourceFlightOwnerV1::new(NodeId::parse("node-b").unwrap(), "session-b").unwrap();
        process.attach_owner(first.clone()).unwrap();
        process.attach_owner(second.clone()).unwrap();
        process
            .clone()
            .terminate_blocking(Duration::from_millis(1))
            .unwrap();

        let rows = process.journal_records().unwrap();
        let owners = rows.iter().find_map(|row| match &row.event {
            crate::retained_resource_flight::ResourceFlightJournalEventV1::IntentJournaled {
                owner_snapshot,
                ..
            } => Some(owner_snapshot.clone()),
            _ => None,
        });
        let owners = owners.expect("intent owner snapshot");
        assert!(owners.contains(&first));
        assert!(owners.contains(&second));
    }

    #[test]
    fn child_first_kill_closes_over_fork_during_sweep() {
        let root = member(42, 7, 0);
        let child = member(43, 8, 42);
        let forked = member(44, 9, 43);
        let fake = Arc::new(FakeProcessAuthority::with_censuses(vec![
            vec![root.clone(), child.clone()],
            vec![root.clone(), child.clone()],
            vec![root.clone(), child.clone(), forked.clone()],
            vec![root.clone(), child.clone(), forked.clone()],
            vec![root.clone(), child.clone(), forked.clone()],
        ]));
        fake.install(root.identity.clone());
        fake.install(child.identity.clone());
        fake.install(forked.identity.clone());
        let (process, _) = fake_owned(fake.clone(), ProcessFlightModeV1::ProtectedV3);
        process
            .terminate_blocking(Duration::from_millis(1))
            .unwrap();
        let killed: Vec<_> = fake
            .signals()
            .into_iter()
            .filter_map(|(pid, signal)| (signal == libc::SIGKILL).then_some(pid))
            .collect();
        assert_eq!(killed, vec![44, 43, 42]);
    }

    #[test]
    fn async_sigstop_delivery_must_verify_stopped_before_matching_census_closes() {
        let root = member(42, 7, 0);
        let child = member(43, 8, 42);
        let forked = member(44, 9, 43);
        let fake = Arc::new(FakeProcessAuthority::with_censuses(vec![
            // signal_term census, then the first matching closure pair.
            vec![root.clone(), child.clone()],
            vec![root.clone(), child.clone()],
            vec![root.clone(), child.clone()],
            // The child kept running through that SIGSTOP volley and forked.
            vec![root.clone(), child.clone(), forked.clone()],
            vec![root.clone(), child.clone(), forked.clone()],
        ]));
        fake.install(root.identity.clone());
        fake.install(child.identity.clone());
        fake.install(forked.identity.clone());
        fake.delay_stop_probe(child.identity.pid, 1);
        let (process, _) = fake_owned(fake.clone(), ProcessFlightModeV1::ProtectedV3);

        process
            .terminate_blocking(Duration::from_millis(1))
            .unwrap();

        let killed: Vec<_> = fake
            .signals()
            .into_iter()
            .filter_map(|(pid, signal)| (signal == libc::SIGKILL).then_some(pid))
            .collect();
        assert_eq!(
            killed,
            vec![44, 43, 42],
            "a matching census cannot close until every member verifies stopped"
        );
    }

    #[test]
    fn live_anchor_admits_reparented_same_group_member() {
        let root = member(42, 7, 0);
        let orphan = member(43, 8, 1);
        let fake = Arc::new(FakeProcessAuthority::with_censuses(vec![
            vec![root.clone(), orphan.clone()],
            vec![root.clone(), orphan.clone()],
            vec![root.clone(), orphan],
        ]));
        fake.install(root.identity.clone());
        fake.install(process_identity(43, 8));
        let (process, _) = fake_owned(fake.clone(), ProcessFlightModeV1::ProtectedV3);

        let result = process
            .terminate_blocking(Duration::from_millis(1))
            .unwrap();

        assert_eq!(result.disposition, ResourceActionDispositionV1::Complete);
        assert_eq!(
            fake.signals()
                .into_iter()
                .filter_map(|(pid, signal)| (signal == libc::SIGKILL).then_some(pid))
                .collect::<Vec<_>>(),
            vec![43, 42]
        );
    }

    #[test]
    fn anchor_loss_with_remaining_same_group_member_refuses_kill() {
        let root = member(42, 7, 0);
        let helper = member(43, 8, 42);
        let fake = Arc::new(FakeProcessAuthority::with_censuses(vec![
            vec![root.clone()],
            vec![helper.clone()],
        ]));
        fake.install(root.identity.clone());
        fake.install(helper.identity.clone());
        fake.remove_identity_on_signal(42, libc::SIGTERM);
        let (process, _) = fake_owned(fake.clone(), ProcessFlightModeV1::ProtectedV3);

        let result = process
            .terminate_blocking(Duration::from_millis(1))
            .unwrap();

        assert_eq!(result.disposition, ResourceActionDispositionV1::Failed);
        assert!(
            !fake
                .signals()
                .iter()
                .any(|(pid, signal)| *pid == 43 && *signal == libc::SIGKILL),
            "an unanchored helper must not be killed"
        );
    }

    #[test]
    fn live_root_with_empty_census_is_a_typed_refusal() {
        let fake = Arc::new(FakeProcessAuthority::default());
        fake.install(process_identity(42, 7));
        let (process, _) = fake_owned(fake.clone(), ProcessFlightModeV1::ProtectedV3);

        let result = process
            .terminate_blocking(Duration::from_millis(1))
            .unwrap();

        assert_eq!(result.disposition, ResourceActionDispositionV1::Failed);
        assert!(!fake
            .signals()
            .iter()
            .any(|(_, signal)| *signal == libc::SIGKILL));
    }

    #[test]
    fn unstable_containment_resumes_every_stopped_member_without_kill() {
        let root = member(42, 7, 0);
        let child = member(43, 8, 42);
        let fake = Arc::new(FakeProcessAuthority::with_censuses(vec![vec![
            root.clone(),
            child.clone(),
        ]]));
        fake.install(root.identity.clone());
        fake.install(child.identity.clone());
        fake.delay_stop_probe(42, CONTAINMENT_CLOSURE_LIMIT + 1);
        let (process, _) = fake_owned(fake.clone(), ProcessFlightModeV1::ProtectedV3);

        let result = process
            .terminate_blocking(Duration::from_millis(1))
            .unwrap();
        let signals = fake.signals();

        assert_eq!(result.disposition, ResourceActionDispositionV1::Failed);
        assert!(!signals.iter().any(|(_, signal)| *signal == libc::SIGKILL));
        assert!(signals.contains(&(42, libc::SIGCONT)));
        assert!(signals.contains(&(43, libc::SIGCONT)));
    }

    #[test]
    fn zombie_and_exited_members_are_contained_before_kill() {
        for exited in [false, true] {
            let root = member(42, 7, 0);
            let child = member(43, 8, 42);
            let fake = Arc::new(FakeProcessAuthority::with_censuses(vec![
                vec![root.clone(), child.clone()],
                vec![root.clone(), child.clone()],
                vec![root.clone(), child.clone()],
            ]));
            fake.install(root.identity.clone());
            fake.install(child.identity.clone());
            if exited {
                fake.remove_identity_on_signal(43, libc::SIGSTOP);
            } else {
                fake.mark_zombie(43);
            }
            let (process, _) = fake_owned(fake.clone(), ProcessFlightModeV1::ProtectedV3);

            let result = process
                .clone()
                .terminate_blocking(Duration::from_millis(1))
                .unwrap();

            assert_eq!(
                result.disposition,
                ResourceActionDispositionV1::Complete,
                "exited={exited}"
            );
            assert!(
                process.journal_records().unwrap().iter().any(|row| matches!(
                    &row.event,
                    crate::retained_resource_flight::ResourceFlightJournalEventV1::ProcessSignalsObserved {
                        observations
                    } if observations.iter().any(|observation| observation.pid == 43
                        && observation.signal == libc::SIGKILL)
                )),
                "contained member remains part of the observed kill volley"
            );
        }
    }

    #[test]
    fn reap_timeout_is_failed_for_v2_and_v3() {
        for mode in [
            ProcessFlightModeV1::LegacyV2,
            ProcessFlightModeV1::ProtectedV3,
        ] {
            let root = member(42, 7, 0);
            let fake = Arc::new(FakeProcessAuthority::with_censuses(vec![
                vec![root.clone()],
            ]));
            fake.install(root.identity.clone());
            let (process, _) = fake_owned_with_wait(
                fake.clone(),
                mode,
                Arc::new(NeverReapedChildWaitV1),
                Duration::ZERO,
            );

            let result = process.terminate_blocking(Duration::ZERO).unwrap();

            assert_eq!(result.disposition, ResourceActionDispositionV1::Failed);
            let cause = result.cause.expect("reap timeout cause");
            assert_eq!(cause.failure_class, DiagnosticFailureClass::Timeout);
            assert_eq!(cause.code.as_str(), "process.reap_timeout");
            if mode == ProcessFlightModeV1::LegacyV2 {
                assert_eq!(
                    fake.group_signals(),
                    vec![
                        (42, libc::SIGTERM),
                        (42, libc::SIGKILL),
                        (42, libc::SIGKILL)
                    ]
                );
            }
        }
    }

    #[test]
    fn not_found_driver_and_joiners_converge_after_settlement() {
        let fake = Arc::new(FakeProcessAuthority::default());
        fake.install(process_identity(42, 7));
        let (process, _) = fake_owned(fake.clone(), ProcessFlightModeV1::ProtectedV3);
        let state = Arc::clone(process.live().unwrap());
        assert!(matches!(
            state.claim_action().unwrap(),
            ActionClaimV1::Drive
        ));
        state.prepare_dispatch().unwrap();
        fake.remove(42);

        let joining = process.clone();
        let (sent, received) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || sent.send(joining.join_blocking()).unwrap());
        let durable = state.settle_dispatch(Vec::new(), None).unwrap();
        let driver = state.finish_action(Ok(durable.clone())).unwrap();

        assert_eq!(
            received
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap(),
            durable
        );
        assert_eq!(driver, durable);
        assert_eq!(process.join_blocking().unwrap(), durable);
    }

    #[test]
    fn not_found_while_driving_joins_the_same_terminal_result() {
        let root = member(42, 7, 0);
        let fake = Arc::new(FakeProcessAuthority::with_censuses(vec![
            vec![root.clone()],
            Vec::new(),
            Vec::new(),
        ]));
        fake.install(root.identity.clone());
        fake.remove_identity_on_signal(42, libc::SIGTERM);
        let gate = fake.gate_signal(42, libc::SIGTERM);
        let not_found = fake.gate_next_not_found();
        let (process, _) = fake_owned(fake, ProcessFlightModeV1::ProtectedV3);

        let driving = process.clone();
        let (driver_tx, driver_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            driver_tx
                .send(driving.terminate_blocking(Duration::ZERO))
                .unwrap();
        });
        gate.entered.wait();

        let joining = process.clone();
        let (join_tx, join_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            join_tx
                .send(joining.terminate_blocking(Duration::ZERO))
                .unwrap();
        });
        not_found.entered.wait();
        not_found.release.wait();
        assert!(
            join_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "the NotFound caller must join while the driver is still signaling"
        );
        gate.release.wait();

        let driver = driver_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        let joiner = join_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(driver, joiner);
        assert_eq!(driver.disposition, ResourceActionDispositionV1::Complete);
    }

    #[test]
    fn recovery_winning_after_dispatch_is_authoritative_everywhere() {
        let fake = Arc::new(FakeProcessAuthority::default());
        fake.install(process_identity(42, 7));
        let journal = Arc::new(RecoveryWinsProcessJournal::new());
        let journal_port: Arc<dyn ResourceFlightJournal> = journal.clone();
        let publisher = Arc::new(RecordingProcessPublisher::default());
        let publisher_port: Arc<dyn ResourceFlightResultPublisher> = publisher.clone();
        let process = fake_owned_with_components(
            fake,
            ProcessFlightModeV1::ProtectedV3,
            Arc::new(ReapedChildWaitV1),
            BLOCKING_TERMINATE_REAP_BOUND,
            journal_port,
            publisher_port,
        );
        let state = Arc::clone(process.live().unwrap());
        let flight_id = state.flight.flight_id().clone();
        assert!(matches!(
            state.claim_action().unwrap(),
            ActionClaimV1::Drive
        ));
        state.prepare_dispatch().unwrap();

        let driving_state = Arc::clone(&state);
        let driver = std::thread::spawn(move || {
            let returned = driving_state.settle_dispatch(Vec::new(), None)?;
            driving_state.finish_action(Ok(returned))
        });
        journal.driver_at_terminal.wait();
        let recovered = RetainedResourceFlight::recover_dead_journaled_intent_as_unknown(
            journal.as_ref(),
            &flight_id,
            17,
            publisher.as_ref(),
        )
        .unwrap()
        .expect("recovery wins terminal CAS");
        journal.recovery_done.wait();
        let driver = driver.join().unwrap().unwrap();
        let joiner = process.join_blocking().unwrap();

        assert_eq!(recovered.disposition, ResourceActionDispositionV1::Unknown);
        assert_eq!(driver, recovered);
        assert_eq!(joiner, recovered);
        let rows = journal.records(&flight_id).unwrap();
        let terminal = rows
            .iter()
            .filter_map(|row| match &row.event {
                crate::retained_resource_flight::ResourceFlightJournalEventV1::Settled {
                    result,
                } => Some(result),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(terminal, vec![&recovered]);
        let publications = publisher.0.lock().unwrap();
        assert_eq!(publications.len(), 1);
        assert_eq!(publications[0].result, recovered);
    }

    #[test]
    fn action_driver_unwind_wakes_a_bare_thread_joiner() {
        let fake = Arc::new(FakeProcessAuthority::default());
        fake.install(process_identity(42, 7));
        let (process, _) = fake_owned(fake, ProcessFlightModeV1::ProtectedV3);
        let state = Arc::clone(process.live().unwrap());
        assert!(matches!(
            state.claim_action().unwrap(),
            ActionClaimV1::Drive
        ));
        let joining = process.clone();
        let (sent, received) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || sent.send(joining.join_blocking()).unwrap());

        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = DrivingActionGuardV1::new(&state);
            panic!("injected action-driver unwind");
        }))
        .is_err());

        assert!(matches!(
            received.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err(ProcessAuthorityErrorV1::Flight(detail))
                if detail.contains("unwound before settlement")
        ));
    }

    #[test]
    fn poisoned_legacy_action_lock_cannot_suppress_drop_sigkill() {
        let fake = Arc::new(FakeProcessAuthority::default());
        fake.install(process_identity(42, 7));
        let (process, _) = fake_owned(fake.clone(), ProcessFlightModeV1::LegacyV2);
        let state = Arc::clone(process.live().unwrap());
        assert!(std::thread::spawn(move || {
            let _guard = state.action.lock().unwrap();
            panic!("poison legacy action lock");
        })
        .join()
        .is_err());

        drop(process);

        assert_eq!(fake.group_signals(), vec![(42, libc::SIGKILL)]);
    }

    #[test]
    fn forced_failing_kill_is_typed_and_journaled() {
        let root = member(42, 7, 0);
        let fake = Arc::new(FakeProcessAuthority::with_censuses(vec![
            vec![root.clone()],
            vec![root.clone()],
            vec![root],
        ]));
        fake.install(process_identity(42, 7));
        *fake.fail_signal.lock().unwrap() = Some(libc::SIGKILL);
        let (process, _) = fake_owned(fake, ProcessFlightModeV1::ProtectedV3);
        let result = process
            .clone()
            .terminate_blocking(Duration::from_millis(1))
            .unwrap();
        assert_eq!(result.disposition, ResourceActionDispositionV1::Failed);
        assert!(process.journal_records().unwrap().iter().any(|row| matches!(
            &row.event,
            crate::retained_resource_flight::ResourceFlightJournalEventV1::ProcessSignalsObserved {
                observations
            } if observations.iter().any(|observation| observation.signal == libc::SIGKILL
                && observation.return_code == -1 && observation.errno == Some(libc::EPERM))
        )));
    }

    #[test]
    fn drop_outside_flight_refuses_in_v3_and_v2_drop_is_one_legacy_group_sigkill() {
        let v3_fake = Arc::new(FakeProcessAuthority::default());
        v3_fake.install(process_identity(42, 7));
        let (v3, v3_journal) = fake_owned(v3_fake.clone(), ProcessFlightModeV1::ProtectedV3);
        let v3_id = v3.live().unwrap().flight.flight_id().clone();
        drop(v3);
        assert!(v3_fake.signals().is_empty());
        assert!(v3_journal
            .records(&v3_id)
            .unwrap()
            .iter()
            .any(|row| matches!(
                &row.event,
                crate::retained_resource_flight::ResourceFlightJournalEventV1::Settled {
                    result: ResourceActionResultV1 {
                        disposition: ResourceActionDispositionV1::Failed,
                        ..
                    }
                }
            )));

        let root = member(42, 7, 0);
        let v2_fake = Arc::new(FakeProcessAuthority::with_censuses(vec![
            vec![root.clone()],
            vec![root.clone()],
            vec![root],
        ]));
        v2_fake.install(process_identity(42, 7));
        let (v2, v2_journal) = fake_owned(v2_fake.clone(), ProcessFlightModeV1::LegacyV2);
        let v2_id = v2.live().unwrap().flight.flight_id().clone();
        drop(v2);
        assert_eq!(
            v2_fake.group_signals(),
            vec![(42, libc::SIGKILL)],
            "V2 Drop must remain the old one-stage process-group backstop"
        );
        assert!(
            v2_fake.signals().is_empty(),
            "V2 Drop must not enter V3 per-process stop-and-rescan containment"
        );
        let rows = v2_journal.records(&v2_id).unwrap();
        let observations = rows
            .iter()
            .find_map(|row| match &row.event {
                crate::retained_resource_flight::ResourceFlightJournalEventV1::ProcessSignalsObserved {
                    observations,
                } => Some(observations),
                _ => None,
            })
            .expect("V2 Drop syscall outcome must be observed");
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].pid, 42);
        assert_eq!(observations[0].signal, libc::SIGKILL);
        assert_eq!(observations[0].return_code, 0);
    }

    #[test]
    fn protected_capacity_refuses_before_spawn_for_caps_two_and_seven() {
        for cap in [2, 7] {
            let root = tempfile::tempdir().unwrap();
            let attempt_id = AttemptId::mint().unwrap();
            let generation = format!("capacity-{cap}-{}", attempt_id.as_str());
            let journal = Arc::new(FileResourceFlightJournal::open(root.path(), cap).unwrap());
            let owner = ResourceFlightOwnerV1::new(
                NodeId::parse("process-spawn").unwrap(),
                generation.clone(),
            )
            .unwrap();
            let durable = DurableProcessFlightV3::new(
                attempt_id.clone(),
                generation,
                Arc::new(ResourceFlightRegistryV1::new(attempt_id)),
                journal,
                owner,
            )
            .unwrap();
            let spawner = Arc::new(CountingSpawnPortV1::default());
            let spawn_port: Arc<dyn ProcessSpawnPortV1> = spawner.clone();

            let error = OwnedProcessTreeV1::spawn_with_authority(
                true_program_path(),
                &[],
                None,
                DiagnosticRedactor::default(),
                None,
                false,
                &[],
                Arc::new(FakeProcessAuthority::default()),
                ProcessFlightProvisionV1::DurableV3(durable),
                spawn_port,
            )
            .unwrap_err();

            assert!(
                error.to_string().contains("full"),
                "cap {cap} must fail lifecycle admission: {error}"
            );
            assert_eq!(
                spawner.calls.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "cap {cap} must refuse before Command::spawn"
            );
        }
    }

    #[test]
    fn durable_attempt_route_reuses_exactly_one_registry_for_every_generation() {
        let root = tempfile::tempdir().unwrap();
        let attempt_id = AttemptId::mint().unwrap();
        let journal = Arc::new(FileResourceFlightJournal::open(root.path(), 512).unwrap());
        let publisher = Arc::new(RecordingProcessPublisher::default());
        let publisher_port: Arc<dyn ResourceFlightResultPublisher> = publisher.clone();
        let route = DurableProcessFlightAttemptV3::new(attempt_id.clone(), journal)
            .with_result_publisher(publisher_port);
        let first = route
            .bind_generation(
                "generation-a",
                ResourceFlightOwnerV1::new(NodeId::parse("node-a").unwrap(), "session-a").unwrap(),
            )
            .unwrap();
        let second = route
            .bind_generation(
                "generation-b",
                ResourceFlightOwnerV1::new(NodeId::parse("node-b").unwrap(), "session-b").unwrap(),
            )
            .unwrap();
        let container = route
            .bind_container_generation(
                "container-generation",
                ResourceFlightOwnerV1::new(
                    NodeId::parse("container-node").unwrap(),
                    "container-session",
                )
                .unwrap(),
            )
            .unwrap();
        assert!(Arc::ptr_eq(&route.registry, &first.registry));
        assert!(Arc::ptr_eq(&first.registry, &second.registry));
        assert!(Arc::ptr_eq(&route.registry, &container.registry));
        assert!(Arc::ptr_eq(&route.journal, &first.journal));
        assert!(Arc::ptr_eq(&first.journal, &second.journal));
        let route_journal = Arc::clone(&route.journal) as Arc<dyn ResourceFlightJournal>;
        assert!(Arc::ptr_eq(&route_journal, &container.journal));
    }

    #[tokio::test]
    async fn journal_then_spawn_fails_and_spawn_then_bind_fails_record_protectively() {
        let spawn_root = tempfile::tempdir().unwrap();
        let spawn_attempt = AttemptId::mint().unwrap();
        let spawn_generation = format!("durable-spawn-{}", spawn_attempt.as_str());
        let spawn_journal =
            Arc::new(FileResourceFlightJournal::open(spawn_root.path(), 512).unwrap());
        let spawn_owner = ResourceFlightOwnerV1::new(
            NodeId::parse("process-spawn").unwrap(),
            spawn_generation.clone(),
        )
        .unwrap();
        let spawn_flight = DurableProcessFlightV3::new(
            spawn_attempt.clone(),
            spawn_generation,
            Arc::new(ResourceFlightRegistryV1::new(spawn_attempt)),
            Arc::clone(&spawn_journal),
            spawn_owner,
        )
        .unwrap();
        let spawn_flight_id = spawn_flight.flight_id().clone();
        let missing_program = spawn_root.path().join("program-that-does-not-exist");
        let spawn_error = OwnedProcessTreeV1::spawn_with_durable_flight_v3(
            missing_program.to_str().unwrap(),
            &[],
            None,
            DiagnosticRedactor::default(),
            None,
            false,
            &[],
            spawn_flight,
        )
        .unwrap_err();
        assert_eq!(spawn_error.kind(), std::io::ErrorKind::NotFound);
        drop(spawn_journal);
        let spawn_journal = FileResourceFlightJournal::open(spawn_root.path(), 512).unwrap();
        let spawn_rows = spawn_journal.records(&spawn_flight_id).unwrap();
        assert!(spawn_rows.iter().any(|row| matches!(
            &row.event,
            crate::retained_resource_flight::ResourceFlightJournalEventV1::ProcessBindingFailed {
                stage: ProcessBindingStageV1::Spawn,
                pid: None,
            }
        )));
        assert!(spawn_rows.iter().any(|row| matches!(
            &row.event,
            crate::retained_resource_flight::ResourceFlightJournalEventV1::ProcessSignalsObserved {
                observations
            } if observations.is_empty()
        )));
        assert!(spawn_rows.iter().any(|row| matches!(
            &row.event,
            crate::retained_resource_flight::ResourceFlightJournalEventV1::Settled {
                result: ResourceActionResultV1 {
                    disposition: ResourceActionDispositionV1::Failed,
                    ..
                }
            }
        )));

        let bind_root = tempfile::tempdir().unwrap();
        let bind_attempt = AttemptId::mint().unwrap();
        let bind_generation = format!("durable-bind-{}", bind_attempt.as_str());
        let bind_journal =
            Arc::new(FileResourceFlightJournal::open(bind_root.path(), 512).unwrap());
        let bind_owner = ResourceFlightOwnerV1::new(
            NodeId::parse("process-spawn").unwrap(),
            bind_generation.clone(),
        )
        .unwrap();
        let bind_flight = DurableProcessFlightV3::new(
            bind_attempt.clone(),
            bind_generation,
            Arc::new(ResourceFlightRegistryV1::new(bind_attempt)),
            Arc::clone(&bind_journal),
            bind_owner,
        )
        .unwrap();
        let bind_flight_id = bind_flight.flight_id().clone();
        let authority: Arc<dyn ProcessAuthorityPortV1> = Arc::new(FakeProcessAuthority::default());
        let bind_error = OwnedProcessTreeV1::spawn_with_authority(
            true_program_path(),
            &[],
            None,
            DiagnosticRedactor::default(),
            None,
            false,
            &[],
            authority,
            ProcessFlightProvisionV1::DurableV3(bind_flight),
            Arc::new(SystemProcessSpawnV1),
        )
        .unwrap_err();
        assert_eq!(bind_error.kind(), std::io::ErrorKind::NotFound);
        drop(bind_journal);
        let bind_journal = FileResourceFlightJournal::open(bind_root.path(), 512).unwrap();
        let bind_rows = bind_journal.records(&bind_flight_id).unwrap();
        assert!(bind_rows.iter().any(|row| matches!(
            &row.event,
            crate::retained_resource_flight::ResourceFlightJournalEventV1::ProcessBindingFailed {
                stage: ProcessBindingStageV1::ImmutableStart,
                pid: Some(_),
            }
        )));
        assert!(bind_rows.iter().any(|row| matches!(
            &row.event,
            crate::retained_resource_flight::ResourceFlightJournalEventV1::ProcessSignalsObserved {
                observations
            } if observations.is_empty()
        )));
        assert!(bind_rows.iter().any(|row| matches!(
            &row.event,
            crate::retained_resource_flight::ResourceFlightJournalEventV1::Settled {
                result: ResourceActionResultV1 {
                    disposition: ResourceActionDispositionV1::Failed,
                    ..
                }
            }
        )));
    }

    #[test]
    fn terminate_blocking_bare_thread_joins_or_refuses_without_hanging() {
        let root = member(42, 7, 0);
        let fake = Arc::new(FakeProcessAuthority::with_censuses(vec![
            vec![root.clone()],
            vec![root.clone()],
            vec![root],
        ]));
        fake.install(process_identity(42, 7));
        let (process, _) = fake_owned(fake, ProcessFlightModeV1::ProtectedV3);
        let (sent, received) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = sent.send(process.terminate_blocking(Duration::from_millis(1)));
        });
        assert!(received
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_ok());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn platform_stopped_state_probe_observes_sigstop_host_signal_semantics() {
        let process = OwnedProcessTreeV1::spawn("/bin/sleep", &["30"], None)
            .expect("spawn stopped-state probe child");
        let authority = SystemProcessAuthorityV1;
        let identity = authority.immutable_start(process.pid()).unwrap();
        let stopped = authority.signal_process(identity.pid, libc::SIGSTOP);
        assert_eq!(stopped.return_code, 0, "SIGSTOP dispatch must succeed");
        let verified = (0..100).any(|_| {
            if authority.process_is_stopped(&identity).unwrap_or(false) {
                true
            } else {
                std::thread::sleep(Duration::from_millis(5));
                false
            }
        });
        let result = process.terminate(Duration::from_millis(1)).await;
        assert!(
            verified,
            "platform status probe never observed stopped state"
        );
        assert!(
            result.is_ok(),
            "stopped probe child must remain flight-reapable"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_proc_start_identity_field_22_is_available_host_signal_semantics() {
        let identity = SystemProcessAuthorityV1
            .immutable_start(std::process::id())
            .unwrap();
        assert_eq!(identity.pid, std::process::id());
        assert_ne!(identity.start_time_ticks, 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn darwin_kern_proc_pid_start_identity_is_available_host_signal_semantics() {
        let identity = SystemProcessAuthorityV1
            .immutable_start(std::process::id())
            .unwrap();
        assert_eq!(identity.pid, std::process::id());
        assert_ne!(identity.start_time_ticks, 0);
    }
}
