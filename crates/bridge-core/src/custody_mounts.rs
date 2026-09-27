//! The ADR-0041 mount-point census, v1 (slice 2B2b2b2).
//!
//! [`mount_points_v1`] lists every mount point this process can see, as raw path bytes. The
//! exporter refuses a plan-backed export when any of them lies strictly inside a protected source
//! root ([`mount_point_within`]). A same-device bind mount there would show a second view of some
//! tree behind an ordinary-looking directory, and the walker's device check cannot see it.
//!
//! - **Linux** reads `/proc/self/mountinfo` through a bounded read and parses it as bytes, never
//!   as UTF-8 ([`parse_mountinfo_v1`]).
//! - **macOS** asks `getfsstat` for the count, then fills an exactly sized buffer, retrying once if
//!   the count moved ([`census_by_count`]).
//! - **Any other unix** refuses [`CustodyMountErrorV1::CensusUnsupported`].
//!
//! Every refusal fails closed: the caller refuses the export. The census reads no source content
//! and writes nothing.

use std::io;

/// The Linux mount table is read to at most this many bytes; a longer one is refused.
pub(crate) const MOUNTINFO_MAX_BYTES_V1: usize = 4 * 1024 * 1024;
/// The macOS census admits at most this many mounted filesystems.
pub(crate) const MAX_MOUNT_POINTS_V1: usize = 4096;

/// Why the census could not list the mount points. Each refusal fails closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CustodyMountErrorV1 {
    /// This platform has no supported census.
    #[error("this platform has no supported mount census")]
    CensusUnsupported,
    /// The mount table exceeds its bound.
    #[error("the mount table exceeds its bound")]
    Oversize,
    /// A mount-table line (1-based) is not a well-formed record.
    #[error("mount table line {line} is not a well-formed record")]
    MalformedLine { line: usize },
    /// A mount point (on a 1-based line) holds an escape other than `\040`, `\011`, `\012`, or
    /// `\134`.
    #[error("mount table line {line} holds a malformed mount-point escape")]
    MalformedEscape { line: usize },
    /// The census listed no mount point at all.
    #[error("the mount census listed no mount point")]
    Empty,
    /// The mounted-filesystem count changed on both attempts.
    #[error("the mounted-filesystem count changed on both attempts")]
    Unstable,
    /// Reading the mount table failed.
    #[error("the mount census io failed: {0}")]
    Io(io::ErrorKind),
}

/// Every mount point this process can see, as raw path bytes. An empty census fails closed.
pub(crate) fn mount_points_v1() -> Result<Vec<Vec<u8>>, CustodyMountErrorV1> {
    #[cfg(test)]
    let mount_points = match seam::census() {
        Some(injected) => injected?,
        None => platform_mount_points()?,
    };
    #[cfg(not(test))]
    let mount_points = platform_mount_points()?;
    if mount_points.is_empty() {
        return Err(CustodyMountErrorV1::Empty);
    }
    Ok(mount_points)
}

/// Whether `mount_point` lies strictly inside `root`: a proper prefix on component boundaries.
/// `/repo/target` lies inside `/repo`; `/repository` and `/repo` itself do not. The root's own
/// mount is at or above the root, so it never counts.
pub(crate) fn mount_point_within(mount_point: &[u8], root: &[u8]) -> bool {
    let root = if root.len() > 1 {
        root.strip_suffix(b"/").unwrap_or(root)
    } else {
        root
    };
    match mount_point.strip_prefix(root) {
        Some(rest) if !rest.is_empty() => root.ends_with(b"/") || rest[0] == b'/',
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------------
// Linux: `/proc/self/mountinfo`
// ---------------------------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn platform_mount_points() -> Result<Vec<Vec<u8>>, CustodyMountErrorV1> {
    use std::io::Read as _;
    let file = std::fs::File::open("/proc/self/mountinfo")
        .map_err(|error| CustodyMountErrorV1::Io(error.kind()))?;
    let mut bytes = Vec::new();
    file.take(MOUNTINFO_MAX_BYTES_V1 as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| CustodyMountErrorV1::Io(error.kind()))?;
    parse_mountinfo_v1(&bytes)
}

/// Parses a `mountinfo` table as bytes and returns field 5 of every record, the mount point, with
/// its octal escapes decoded.
///
/// A record is `id parent major:minor root mount-point options [optional...] - type source
/// super-options`, separated by single spaces and ended by a newline. The kernel escapes a space,
/// tab, newline, or backslash in the mount point as `\040`, `\011`, `\012`, or `\134`; every
/// other byte, a non-UTF-8 byte included, is the path's own. Any other escape, a malformed
/// record, or a table over [`MOUNTINFO_MAX_BYTES_V1`] refuses.
pub(crate) fn parse_mountinfo_v1(bytes: &[u8]) -> Result<Vec<Vec<u8>>, CustodyMountErrorV1> {
    if bytes.len() > MOUNTINFO_MAX_BYTES_V1 {
        return Err(CustodyMountErrorV1::Oversize);
    }
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let Some(body) = bytes.strip_suffix(b"\n") else {
        let lines = bytes.split(|byte| *byte == b'\n').count();
        return Err(CustodyMountErrorV1::MalformedLine { line: lines });
    };
    body.split(|byte| *byte == b'\n')
        .enumerate()
        .map(|(index, line)| mountinfo_mount_point(line, index + 1))
        .collect()
}

/// The decoded mount point of one record, `number` being its 1-based line.
fn mountinfo_mount_point(line: &[u8], number: usize) -> Result<Vec<u8>, CustodyMountErrorV1> {
    let malformed = CustodyMountErrorV1::MalformedLine { line: number };
    let fields: Vec<&[u8]> = line.split(|byte| *byte == b' ').collect();
    // The optional fields end at a lone `-`, which three fields follow.
    let separator = fields
        .iter()
        .skip(6)
        .position(|field| *field == b"-")
        .map(|offset| offset + 6)
        .ok_or(malformed)?;
    let well_formed = is_decimal(fields[0])
        && is_decimal(fields[1])
        && fields[2]
            .split(|byte| *byte == b':')
            .map(is_decimal)
            .eq([true, true])
        && !fields[3].is_empty()
        && !fields[5].is_empty()
        && fields.len() >= separator + 4;
    if !well_formed {
        return Err(malformed);
    }
    let mount_point = decode_mount_point(fields[4])
        .ok_or(CustodyMountErrorV1::MalformedEscape { line: number })?;
    if !mount_point.starts_with(b"/") {
        return Err(malformed);
    }
    Ok(mount_point)
}

fn is_decimal(field: &[u8]) -> bool {
    !field.is_empty() && field.iter().all(u8::is_ascii_digit)
}

/// Decodes exactly the four escapes the kernel writes in a mount point. Every other byte passes
/// through verbatim. `None` means a backslash starts anything else.
fn decode_mount_point(field: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(field.len());
    let mut rest = field;
    while let Some((&byte, tail)) = rest.split_first() {
        if byte != b'\\' {
            decoded.push(byte);
            rest = tail;
            continue;
        }
        let (code, tail) = (tail.get(..3)?, tail.get(3..)?);
        decoded.push(match code {
            b"040" => b' ',
            b"011" => b'\t',
            b"012" => b'\n',
            b"134" => b'\\',
            _ => return None,
        });
        rest = tail;
    }
    Some(decoded)
}

// ---------------------------------------------------------------------------------------------
// macOS: `getfsstat`
// ---------------------------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn platform_mount_points() -> Result<Vec<Vec<u8>>, CustodyMountErrorV1> {
    census_by_count(getfsstat_count, getfsstat_fill)
}

/// The two-call census: `count` reports the mounted filesystems, and `fill` lists exactly that
/// many into an exactly sized buffer, returning the records it filled. The census stands only if
/// the fill returned the counted number of records and a second count agrees, so a mount that
/// appeared or vanished between the calls is seen. One changed count is retried; a second refuses.
pub(crate) fn census_by_count(
    mut count: impl FnMut() -> Result<usize, CustodyMountErrorV1>,
    mut fill: impl FnMut(usize) -> Result<Vec<Vec<u8>>, CustodyMountErrorV1>,
) -> Result<Vec<Vec<u8>>, CustodyMountErrorV1> {
    for _attempt in 0..2 {
        let counted = count()?;
        if counted > MAX_MOUNT_POINTS_V1 {
            return Err(CustodyMountErrorV1::Oversize);
        }
        let filled = fill(counted)?;
        if filled.len() == counted && count()? == counted {
            return Ok(filled);
        }
    }
    Err(CustodyMountErrorV1::Unstable)
}

/// `getfsstat(NULL, 0, MNT_NOWAIT)`: the number of mounted filesystems.
#[cfg(target_os = "macos")]
fn getfsstat_count() -> Result<usize, CustodyMountErrorV1> {
    // SAFETY: a null buffer of size zero asks only for the number of mounted filesystems, and
    // `getfsstat` writes nothing through it.
    let counted = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    usize::try_from(counted).map_err(|_| CustodyMountErrorV1::Io(io::Error::last_os_error().kind()))
}

/// `getfsstat` into a buffer of exactly `counted` records, and each filled record's
/// `f_mntonname`, up to its terminating NUL.
#[cfg(target_os = "macos")]
fn getfsstat_fill(counted: usize) -> Result<Vec<Vec<u8>>, CustodyMountErrorV1> {
    // SAFETY: `statfs` holds only integers and arrays of integers, so all-zero bytes are a valid
    // value of it.
    let zeroed: libc::statfs = unsafe { std::mem::zeroed() };
    let mut records = vec![zeroed; counted];
    let buffer_bytes = counted
        .checked_mul(std::mem::size_of::<libc::statfs>())
        .and_then(|bytes| libc::c_int::try_from(bytes).ok())
        .ok_or(CustodyMountErrorV1::Oversize)?;
    // SAFETY: `records` holds exactly `counted` initialized records, `buffer_bytes` is their exact
    // size in bytes, and `getfsstat` writes at most `buffer_bytes` bytes into the buffer.
    let filled = unsafe { libc::getfsstat(records.as_mut_ptr(), buffer_bytes, libc::MNT_NOWAIT) };
    let filled = usize::try_from(filled)
        .map_err(|_| CustodyMountErrorV1::Io(io::Error::last_os_error().kind()))?;
    records.truncate(filled);
    records
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let name = &record.f_mntonname;
            let length = name
                .iter()
                .position(|byte| *byte == 0)
                .ok_or(CustodyMountErrorV1::MalformedLine { line: index + 1 })?;
            Ok(name[..length].iter().map(|byte| *byte as u8).collect())
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Other unix
// ---------------------------------------------------------------------------------------------

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn platform_mount_points() -> Result<Vec<Vec<u8>>, CustodyMountErrorV1> {
    Err(CustodyMountErrorV1::CensusUnsupported)
}

// ---------------------------------------------------------------------------------------------
// Test seam
// ---------------------------------------------------------------------------------------------

/// The census's test-only seam: an injected platform census result, chosen per call, and a count
/// of the census calls made while it is installed. [`mount_points_v1`]'s own checks still apply.
#[cfg(test)]
pub(crate) mod seam {
    use super::{parse_mountinfo_v1, CustodyMountErrorV1};
    use std::cell::RefCell;

    /// Given the 1-based census call number, the platform census result to return.
    pub(crate) type CensusV1 = Box<dyn FnMut(usize) -> Result<Vec<Vec<u8>>, CustodyMountErrorV1>>;

    struct StateV1 {
        census: CensusV1,
        calls: usize,
    }

    thread_local! {
        static STATE: RefCell<Option<StateV1>> = const { RefCell::new(None) };
    }

    /// Uninstalls the seam when dropped.
    pub(crate) struct InstalledV1(());

    impl InstalledV1 {
        /// The census calls made since the seam was installed.
        pub(crate) fn calls(&self) -> usize {
            STATE.with(|state| state.borrow().as_ref().map_or(0, |state| state.calls))
        }
    }

    impl Drop for InstalledV1 {
        fn drop(&mut self) {
            STATE.with(|state| *state.borrow_mut() = None);
        }
    }

    pub(crate) fn install(census: CensusV1) -> InstalledV1 {
        STATE.with(|state| *state.borrow_mut() = Some(StateV1 { census, calls: 0 }));
        InstalledV1(())
    }

    /// The same list on every call.
    pub(crate) fn install_list(mount_points: Vec<Vec<u8>>) -> InstalledV1 {
        install(Box::new(move |_| Ok(mount_points.clone())))
    }

    /// The same `mountinfo` text on every call, parsed by the production parser.
    pub(crate) fn install_mountinfo(text: Vec<u8>) -> InstalledV1 {
        install(Box::new(move |_| parse_mountinfo_v1(&text)))
    }

    pub(super) fn census() -> Option<Result<Vec<Vec<u8>>, CustodyMountErrorV1>> {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let state = state.as_mut()?;
            state.calls += 1;
            Some((state.census)(state.calls))
        })
    }
}

#[cfg(test)]
#[path = "custody_mounts_tests.rs"]
mod tests;
