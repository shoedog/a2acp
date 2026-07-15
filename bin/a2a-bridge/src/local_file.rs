//! Pinned, bounded local-file snapshots for security-sensitive CLI evidence.
//!
//! The descriptor is opened before type/size inspection, so a FIFO cannot park the process before the
//! regular-file gate. On Unix, `O_NOFOLLOW` rejects a final symlink and `O_NONBLOCK` makes every special
//! file return promptly; descriptor/path identity is then compared before the canonical path is trusted.

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::BoxError;

#[derive(Debug)]
pub(crate) struct LocalFileSnapshot {
    pub(crate) canonical_path: PathBuf,
    pub(crate) bytes: Vec<u8>,
    pub(crate) sha256: String,
}

fn open_read_only_nonblocking(path: &Path) -> Result<File, std::io::Error> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options.open(path)
}

#[cfg(unix)]
fn same_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
        && left.created().ok() == right.created().ok()
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    let mut out = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

pub(crate) fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn read_regular_file_bounded(
    path: &Path,
    label: &str,
    max_bytes: u64,
) -> Result<LocalFileSnapshot, BoxError> {
    read_regular_file_bounded_after_open(path, label, max_bytes, || {})
}

fn read_regular_file_bounded_after_open<F>(
    path: &Path,
    label: &str,
    max_bytes: u64,
    after_open: F,
) -> Result<LocalFileSnapshot, BoxError>
where
    F: FnOnce(),
{
    if path.as_os_str().is_empty() {
        return Err(format!("{label}: path must be non-empty").into());
    }
    let mut file = open_read_only_nonblocking(path)
        .map_err(|error| format!("{label}: cannot open {}: {error}", path.display()))?;
    after_open();
    let descriptor_metadata = file
        .metadata()
        .map_err(|error| format!("{label}: cannot inspect {}: {error}", path.display()))?;
    if !descriptor_metadata.is_file() {
        return Err(format!("{label}: {} must be a regular file", path.display()).into());
    }
    if descriptor_metadata.len() > max_bytes {
        return Err(format!(
            "{label}: {} exceeds the {max_bytes}-byte limit",
            path.display()
        )
        .into());
    }

    let canonical_path = std::fs::canonicalize(path)
        .map_err(|error| format!("{label}: cannot resolve {}: {error}", path.display()))?;
    let path_metadata = std::fs::metadata(&canonical_path).map_err(|error| {
        format!(
            "{label}: cannot inspect resolved path {}: {error}",
            canonical_path.display()
        )
    })?;
    if !path_metadata.is_file() || !same_file(&descriptor_metadata, &path_metadata) {
        return Err(format!("{label}: path changed while it was being opened").into());
    }

    let mut bytes = Vec::with_capacity(descriptor_metadata.len() as usize);
    file.by_ref()
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{label}: read failed: {error}"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!("{label}: content exceeds the {max_bytes}-byte limit").into());
    }
    let sha256 = sha256_hex(&bytes);
    Ok(LocalFileSnapshot {
        canonical_path,
        bytes,
        sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn bounded_reader_accepts_regular_file_and_hashes_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("evidence.json");
        fs::write(&path, b"abc").unwrap();
        let snapshot = read_regular_file_bounded(&path, "evidence", 3).unwrap();
        assert_eq!(snapshot.bytes, b"abc");
        assert_eq!(
            snapshot.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(snapshot.canonical_path, fs::canonicalize(path).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn bounded_reader_rejects_symlink_fifo_device_and_socket_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt as _;
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let regular = dir.path().join("regular");
        fs::write(&regular, b"ok").unwrap();
        let symlink = dir.path().join("symlink");
        std::os::unix::fs::symlink(&regular, &symlink).unwrap();
        assert!(read_regular_file_bounded(&symlink, "source", 1024).is_err());

        let fifo = dir.path().join("fifo");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `fifo_c` is a valid NUL-terminated path and the mode is a normal permission mask.
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        assert!(read_regular_file_bounded(&fifo, "source", 1024).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));

        assert!(read_regular_file_bounded(Path::new("/dev/null"), "source", 1024).is_err());

        let socket = dir.path().join("socket");
        let _listener = UnixListener::bind(&socket).unwrap();
        assert!(read_regular_file_bounded(&socket, "source", 1024).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn bounded_reader_rejects_path_replacement_after_descriptor_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("evidence.json");
        let original = dir.path().join("original.json");
        fs::write(&path, b"trusted").unwrap();

        let error = read_regular_file_bounded_after_open(&path, "evidence", 1024, || {
            fs::rename(&path, &original).unwrap();
            fs::write(&path, b"replacement").unwrap();
        })
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("path changed while it was being opened"));
    }
}
