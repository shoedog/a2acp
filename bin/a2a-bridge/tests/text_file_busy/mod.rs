//! Test-only `ETXTBSY` warm-up for scripts a test writes and a CLI child later executes. This is a
//! small per-crate copy of `bridge-core`'s `retry_on_text_file_busy`; no crate shares it.
//!
//! A test that writes a script races every other test thread's fork: a child forked while the
//! script's write descriptor is open keeps a writable duplicate until its own exec closes it
//! (`O_CLOEXEC`), and the kernel refuses to exec the script meanwhile (`ETXTBSY`). The window
//! closes by itself. One exec that is not refused proves no writer remains, and none can appear
//! later because the script is never opened for writing again.

use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Retry `operation` while it fails with `ETXTBSY`, up to 50 attempts 10 ms apart. Any other
/// outcome, or the last `ETXTBSY` once the attempts are spent, is returned unchanged.
pub fn retry_on_text_file_busy<T>(operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    retry_on_text_file_busy_bounded(50, Duration::from_millis(10), operation)
}

fn retry_on_text_file_busy_bounded<T>(
    attempts: u32,
    pause: Duration,
    mut operation: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    let mut attempt = 1;
    loop {
        match operation() {
            Err(error)
                if error.kind() == io::ErrorKind::ExecutableFileBusy && attempt < attempts =>
            {
                attempt += 1;
                std::thread::sleep(pause);
            }
            outcome => return outcome,
        }
    }
}

/// Execute a just-written script once with no arguments and a closed stdin, retrying `ETXTBSY`.
/// A caller whose script records its invocation removes that record afterwards.
pub fn warm_up_script(path: &Path) {
    retry_on_text_file_busy(|| Command::new(path).output())
        .expect("warm-up exec of the written script");
}

#[test]
fn text_file_busy_retry_is_bounded_and_retries_only_etxtbsy() {
    let busy = || io::Error::from(io::ErrorKind::ExecutableFileBusy);
    let mut calls = 0;
    let outcome = retry_on_text_file_busy_bounded(50, Duration::ZERO, || {
        calls += 1;
        if calls <= 3 {
            Err(busy())
        } else {
            Ok(calls)
        }
    });
    assert_eq!(outcome.unwrap(), 4);

    let mut calls = 0;
    let outcome: io::Result<()> = retry_on_text_file_busy_bounded(7, Duration::ZERO, || {
        calls += 1;
        Err(busy())
    });
    assert_eq!(
        outcome.unwrap_err().kind(),
        io::ErrorKind::ExecutableFileBusy
    );
    assert_eq!(calls, 7);

    let mut calls = 0;
    let outcome: io::Result<()> = retry_on_text_file_busy(|| {
        calls += 1;
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    });
    assert_eq!(outcome.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(calls, 1);
}
