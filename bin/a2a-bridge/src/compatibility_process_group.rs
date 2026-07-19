//! Retained bridge-owned process-group anchors shared by compatibility resolution and scheduling.
//!
//! A live member prevents a Unix process-group identity from being recycled. Every group signal first
//! revalidates that exact anchor PID and start identity; a stale numeric PGID is never sufficient authority.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ProcessStartMarkerV1 {
    LinuxBootTicks { boot_id: String, start_ticks: u64 },
    MacosEpochMicros { seconds: u64, microseconds: u64 },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ProcessIdentityV1 {
    pub(super) pid: i32,
    pub(super) parent_pid: i32,
    pub(super) process_group: i32,
    pub(super) session_id: i32,
    pub(super) start: ProcessStartMarkerV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AnchorDropPolicy {
    /// Resolver cancellation retains the R3c fail-safe: synchronously kill the still-anchored group.
    KillGroup,
    /// Scheduler cancellation releases only its own anchor. Recovery then holds rather than guessing.
    ReleaseOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AnchoredGroupSignal {
    Term,
    Kill,
}

impl AnchoredGroupSignal {
    fn raw(self) -> libc::c_int {
        match self {
            Self::Term => libc::SIGTERM,
            Self::Kill => libc::SIGKILL,
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_boot_id() -> std::io::Result<String> {
    let file = std::fs::File::open("/proc/sys/kernel/random/boot_id")?;
    let mut raw = String::new();
    let mut bounded = std::io::Read::take(file, 64);
    std::io::Read::read_to_string(&mut bounded, &mut raw)?;
    let value = raw.trim_end_matches('\n');
    if value.len() != 36
        || value.bytes().enumerate().any(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte != b'-'
            } else {
                !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte)
            }
        })
        || !value
            .bytes()
            .any(|byte| matches!(byte, b'1'..=b'9' | b'a'..=b'f'))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Linux boot identity is malformed",
        ));
    }
    Ok(value.to_owned())
}

#[cfg(target_os = "linux")]
fn linux_process_identity(pid: libc::pid_t) -> std::io::Result<Option<ProcessIdentityV1>> {
    if pid <= 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "process pid must be positive",
        ));
    }
    let path = format!("/proc/{pid}/stat");
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut raw = String::new();
    let mut bounded = std::io::Read::take(file, 8193);
    std::io::Read::read_to_string(&mut bounded, &mut raw)?;
    if raw.len() > 8192 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Linux process stat exceeds the bound",
        ));
    }
    let close = raw.rfind(')').ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Linux process stat has no command terminator",
        )
    })?;
    let parsed_pid = raw[..raw.find('(').unwrap_or(close)]
        .trim()
        .parse::<i32>()
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Linux process stat has an invalid pid",
            )
        })?;
    if parsed_pid != pid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Linux process stat pid changed",
        ));
    }
    let fields = raw[close + 1..].split_whitespace().collect::<Vec<_>>();
    if fields.len() < 20 || fields[0].len() != 1 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Linux process stat is truncated",
        ));
    }
    let parse_i32 = |index: usize, label: &str| {
        fields[index].parse::<i32>().map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Linux process stat has an invalid {label}"),
            )
        })
    };
    let start_ticks = fields[19].parse::<u64>().map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Linux process stat has an invalid start time",
        )
    })?;
    if start_ticks == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Linux process stat has a zero start time",
        ));
    }
    Ok(Some(ProcessIdentityV1 {
        pid,
        parent_pid: parse_i32(1, "parent pid")?,
        process_group: parse_i32(2, "process group")?,
        session_id: parse_i32(3, "session id")?,
        start: ProcessStartMarkerV1::LinuxBootTicks {
            boot_id: linux_boot_id()?,
            start_ticks,
        },
    }))
}

#[cfg(target_os = "macos")]
fn macos_process_identity(pid: libc::pid_t) -> std::io::Result<Option<ProcessIdentityV1>> {
    if pid <= 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "process pid must be positive",
        ));
    }
    fn info(pid: libc::pid_t) -> std::io::Result<Option<libc::proc_bsdinfo>> {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        // SAFETY: the output points to a correctly sized writable proc_bsdinfo value and remains
        // live for the call. A short/zero result is treated as disappearance, not partial identity.
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int,
            )
        };
        if read == 0 {
            let error = std::io::Error::last_os_error();
            return if matches!(error.raw_os_error(), Some(libc::ESRCH)) {
                Ok(None)
            } else {
                Err(error)
            };
        }
        if read != std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "macOS process identity is truncated",
            ));
        }
        // SAFETY: proc_pidinfo returned the complete initialized structure size.
        Ok(Some(unsafe { info.assume_init() }))
    }

    let first = match info(pid)? {
        Some(info) => info,
        None => return Ok(None),
    };
    // SAFETY: getsid only reads kernel process metadata for the positive pid.
    let session_id = unsafe { libc::getsid(pid) };
    if session_id <= 0 {
        let error = std::io::Error::last_os_error();
        return if matches!(error.raw_os_error(), Some(libc::ESRCH)) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    let second = match info(pid)? {
        Some(info) => info,
        None => return Ok(None),
    };
    if first.pbi_pid != pid as u32
        || second.pbi_pid != pid as u32
        || first.pbi_ppid != second.pbi_ppid
        || first.pbi_pgid != second.pbi_pgid
        || first.pbi_start_tvsec != second.pbi_start_tvsec
        || first.pbi_start_tvusec != second.pbi_start_tvusec
        || first.pbi_start_tvusec >= 1_000_000
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "macOS process identity changed while observed",
        ));
    }
    Ok(Some(ProcessIdentityV1 {
        pid,
        parent_pid: first.pbi_ppid as i32,
        process_group: first.pbi_pgid as i32,
        session_id,
        start: ProcessStartMarkerV1::MacosEpochMicros {
            seconds: first.pbi_start_tvsec,
            microseconds: first.pbi_start_tvusec,
        },
    }))
}

pub(super) fn process_identity(pid: libc::pid_t) -> std::io::Result<Option<ProcessIdentityV1>> {
    #[cfg(target_os = "linux")]
    {
        linux_process_identity(pid)
    }
    #[cfg(target_os = "macos")]
    {
        macos_process_identity(pid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "exact process identity is supported only on Linux and macOS",
        ))
    }
}

pub(super) fn process_group_members(
    process_group: libc::pid_t,
) -> std::io::Result<Vec<ProcessIdentityV1>> {
    if process_group <= 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "process group must be positive",
        ));
    }
    #[cfg(target_os = "linux")]
    let pids = {
        let mut values = Vec::new();
        for entry in std::fs::read_dir("/proc")? {
            let entry = entry?;
            let Some(raw) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if let Ok(pid) = raw.parse::<libc::pid_t>() {
                if pid > 0 {
                    values.push(pid);
                }
            }
        }
        values
    };
    #[cfg(target_os = "macos")]
    let pids = {
        // SAFETY: a null/zero query returns the current count. The second call receives a writable
        // pid_t buffer with its exact byte size; the kernel cannot write beyond that bound.
        let count = unsafe { libc::proc_listpgrppids(process_group, std::ptr::null_mut(), 0) };
        if count == 0 {
            return Ok(Vec::new());
        }
        if !(0..=1_000_000).contains(&count) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "macOS process inventory count is unavailable or unbounded",
            ));
        }
        let mut values = vec![0 as libc::pid_t; count as usize];
        let bytes =
            i32::try_from(values.len() * std::mem::size_of::<libc::pid_t>()).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "macOS process inventory byte size overflows",
                )
            })?;
        // SAFETY: `values` is initialized writable storage of exactly `bytes` bytes.
        let returned =
            unsafe { libc::proc_listpgrppids(process_group, values.as_mut_ptr().cast(), bytes) };
        if returned == 0 {
            return Ok(Vec::new());
        }
        if returned < 0 || returned as usize > values.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "macOS process inventory changed beyond its captured bound",
            ));
        }
        values.truncate(returned as usize);
        values
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let pids: Vec<libc::pid_t> = {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "process-group enumeration is supported only on Linux and macOS",
        ));
    };

    let mut members = Vec::new();
    for pid in pids {
        match process_identity(pid) {
            Ok(Some(identity)) if identity.process_group == process_group => members.push(identity),
            Ok(Some(_)) | Ok(None) => {}
            Err(error) if matches!(error.raw_os_error(), Some(libc::ESRCH)) => {}
            Err(error) => return Err(error),
        }
    }
    members.sort_by_key(|identity| (identity.pid, identity.parent_pid));
    Ok(members)
}

fn capture_spawned_identity(pid: libc::pid_t) -> std::io::Result<ProcessIdentityV1> {
    let mut last_error = None;
    for _ in 0..32 {
        match process_identity(pid) {
            Ok(Some(identity)) => return Ok(identity),
            Ok(None) => {}
            Err(error) => last_error = Some(error),
        }
        std::thread::yield_now();
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "spawned anchor identity is unavailable",
        )
    }))
}

#[cfg(unix)]
pub(super) struct AnchoredProcessGroup {
    process_group: libc::pid_t,
    anchor_identity: Option<ProcessIdentityV1>,
    anchor: Option<tokio::process::Child>,
    anchor_stdin: Option<tokio::process::ChildStdin>,
    drop_policy: AnchorDropPolicy,
    signal_attempts: u32,
    identity_observer: fn(libc::pid_t) -> std::io::Result<Option<ProcessIdentityV1>>,
}

#[cfg(unix)]
impl AnchoredProcessGroup {
    pub(super) fn start_leader(drop_policy: AnchorDropPolicy) -> std::io::Result<Self> {
        Self::spawn_anchor(None, None, drop_policy)
    }

    pub(super) fn anchor_existing_group(
        process_group: libc::pid_t,
        expected_session: libc::pid_t,
        drop_policy: AnchorDropPolicy,
    ) -> std::io::Result<Self> {
        if process_group <= 0 || expected_session <= 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "anchored process group and session must be positive",
            ));
        }
        Self::spawn_anchor(Some(process_group), Some(expected_session), drop_policy)
    }

    fn spawn_anchor(
        process_group: Option<libc::pid_t>,
        expected_session: Option<libc::pid_t>,
        drop_policy: AnchorDropPolicy,
    ) -> std::io::Result<Self> {
        use std::os::unix::process::CommandExt as _;

        let mut command = tokio::process::Command::new("/bin/cat");
        command
            .current_dir("/")
            .env_clear()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        if drop_policy == AnchorDropPolicy::ReleaseOnly {
            // The R3d1 supervisor must retain its group capability throughout TERM grace. Set the
            // ignored disposition in the forked child before exec; SIG_IGN survives exec, so there
            // is no shell/readiness race in which TERM could kill the anchor first.
            // SAFETY: pre_exec runs in the forked child. `signal` changes only that child and is an
            // async-signal-safe libc operation for this fixed signal/disposition pair.
            unsafe {
                command.as_std_mut().pre_exec(|| {
                    if libc::signal(libc::SIGTERM, libc::SIG_IGN) == libc::SIG_ERR {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        command
            .as_std_mut()
            .process_group(process_group.unwrap_or(0));
        let mut anchor = command.spawn()?;
        let anchor_pid = anchor.id().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "spawned process-group anchor has no pid",
            )
        })?;
        let anchor_pid = libc::pid_t::try_from(anchor_pid).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "spawned process-group anchor pid does not fit pid_t",
            )
        })?;
        let identity = match capture_spawned_identity(anchor_pid) {
            Ok(identity) => identity,
            Err(error) => {
                let _ = anchor.start_kill();
                return Err(error);
            }
        };
        let expected_group = process_group.unwrap_or(anchor_pid);
        if identity.process_group != expected_group
            || expected_session.is_some_and(|session| identity.session_id != session)
        {
            let _ = anchor.start_kill();
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "spawned anchor did not join the exact expected process group and session",
            ));
        }
        let anchor_stdin = anchor.stdin.take().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "spawned process-group anchor has no retained stdin",
            )
        })?;
        Ok(Self {
            process_group: expected_group,
            anchor_identity: Some(identity),
            anchor: Some(anchor),
            anchor_stdin: Some(anchor_stdin),
            drop_policy,
            signal_attempts: 0,
            identity_observer: process_identity,
        })
    }

    pub(super) fn process_group(&self) -> libc::pid_t {
        self.process_group
    }

    pub(super) fn anchor_identity(&self) -> &ProcessIdentityV1 {
        self.anchor_identity
            .as_ref()
            .expect("retained anchor identity is present before final release")
    }

    pub(super) fn anchor_is_retained(&self) -> bool {
        self.anchor.is_some() && self.anchor_identity.is_some()
    }

    pub(super) fn anchor_is_exactly_live(&self) -> std::io::Result<bool> {
        let Some(expected) = &self.anchor_identity else {
            return Ok(false);
        };
        Ok((self.identity_observer)(expected.pid)?.as_ref() == Some(expected))
    }

    pub(super) fn signal(&mut self, signal: AnchoredGroupSignal) -> std::io::Result<()> {
        let Some(expected) = &self.anchor_identity else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "process-group anchor has already been released",
            ));
        };
        if self.anchor.is_none()
            || expected.process_group != self.process_group
            || expected.pid <= 0
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "exact process-group anchor capability is no longer retained",
            ));
        }
        self.signal_attempts = self.signal_attempts.saturating_add(1);
        // SAFETY: the exact bridge-owned Child handle has not been waited/reaped, so its captured PID
        // and PGID cannot be recycled even if the anchor has exited into an unreaped state. This
        // retained capability is intentionally stronger than a late /proc/proc_pidinfo observation:
        // resolver cleanup must not lose descendant containment merely because that observation fails.
        if unsafe { libc::kill(-self.process_group, signal.raw()) } == -1 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) async fn release_and_reap(&mut self) -> std::io::Result<()> {
        if self.anchor.is_none() {
            self.anchor_stdin = None;
            self.anchor_identity = None;
            return Ok(());
        }
        self.anchor_stdin.take();
        if let Some(mut anchor) = self.anchor.take() {
            anchor.wait().await?;
        }
        self.anchor_identity = None;
        Ok(())
    }

    #[cfg(test)]
    fn signal_attempts(&self) -> u32 {
        self.signal_attempts
    }

    #[cfg(test)]
    fn set_identity_observer(
        &mut self,
        observer: fn(libc::pid_t) -> std::io::Result<Option<ProcessIdentityV1>>,
    ) {
        self.identity_observer = observer;
    }
}

#[cfg(unix)]
impl Drop for AnchoredProcessGroup {
    fn drop(&mut self) {
        if self.anchor.is_none() {
            return;
        }
        if self.drop_policy == AnchorDropPolicy::KillGroup
            && self.anchor.is_some()
            && self.anchor_identity.as_ref().is_some_and(|expected| {
                expected.pid > 0 && expected.process_group == self.process_group
            })
        {
            // SAFETY: the exact anchor Child has not been waited/reaped, so the PGID cannot have been
            // recycled. This is the resolver's synchronous cancellation fail-safe; the scheduler uses
            // ReleaseOnly and recovery holds.
            unsafe {
                libc::kill(-self.process_group, libc::SIGKILL);
            }
        }
        self.anchor_stdin.take();
        if let Some(anchor) = &mut self.anchor {
            let _ = anchor.start_kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn retained_anchor_keeps_group_identity_until_final_release() {
        let mut group = AnchoredProcessGroup::start_leader(AnchorDropPolicy::KillGroup).unwrap();
        let identity = group.anchor_identity().clone();
        assert!(group.anchor_is_exactly_live().unwrap());
        assert_eq!(identity.process_group, group.process_group());
        group.release_and_reap().await.unwrap();
        assert!(!group.anchor_is_retained());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repeated_finalization_does_not_signal_again() {
        let mut group = AnchoredProcessGroup::start_leader(AnchorDropPolicy::KillGroup).unwrap();
        group.release_and_reap().await.unwrap();
        group.release_and_reap().await.unwrap();
        assert_eq!(group.signal_attempts(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exact_anchor_authorizes_term_before_release_but_never_after() {
        let mut group = AnchoredProcessGroup::start_leader(AnchorDropPolicy::ReleaseOnly).unwrap();
        group.signal(AnchoredGroupSignal::Term).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(
            group.anchor_is_exactly_live().unwrap(),
            "supervisor anchor must retain the group identity throughout TERM grace"
        );
        group.release_and_reap().await.unwrap();
        assert!(group.signal(AnchoredGroupSignal::Kill).is_err());
        assert_eq!(group.signal_attempts(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retained_child_capability_survives_late_identity_observation_failure() {
        fn failed_observer(_pid: libc::pid_t) -> std::io::Result<Option<ProcessIdentityV1>> {
            Err(std::io::Error::from_raw_os_error(libc::EMFILE))
        }

        let mut group = AnchoredProcessGroup::start_leader(AnchorDropPolicy::KillGroup).unwrap();
        let mut workload = tokio::process::Command::new("/bin/cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .process_group(group.process_group())
            .spawn()
            .unwrap();
        group.set_identity_observer(failed_observer);
        assert_eq!(
            group.anchor_is_exactly_live().unwrap_err().raw_os_error(),
            Some(libc::EMFILE)
        );

        group.signal(AnchoredGroupSignal::Kill).unwrap();
        workload.wait().await.unwrap();
        group.release_and_reap().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn existing_group_anchor_requires_the_exact_session() {
        let mut leader = tokio::process::Command::new("/bin/cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = leader.id().unwrap() as i32;
        let identity = capture_spawned_identity(pid).unwrap();
        let mut anchor = AnchoredProcessGroup::anchor_existing_group(
            identity.process_group,
            identity.session_id,
            AnchorDropPolicy::ReleaseOnly,
        )
        .unwrap();
        assert_eq!(anchor.process_group(), identity.process_group);
        assert_eq!(anchor.anchor_identity().session_id, identity.session_id);
        let wrong_session = if identity.session_id == i32::MAX {
            identity.session_id - 1
        } else {
            identity.session_id + 1
        };
        assert!(AnchoredProcessGroup::anchor_existing_group(
            identity.process_group,
            wrong_session,
            AnchorDropPolicy::ReleaseOnly,
        )
        .is_err());
        let _ = leader.start_kill();
        let _ = leader.wait().await;
        anchor.release_and_reap().await.unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn current_process_identity_is_exact_and_positive() {
        let identity = process_identity(std::process::id() as i32)
            .unwrap()
            .unwrap();
        assert_eq!(identity.pid, std::process::id() as i32);
        assert!(identity.parent_pid > 0);
        assert!(identity.process_group > 0);
        assert!(identity.session_id > 0);
    }
}
