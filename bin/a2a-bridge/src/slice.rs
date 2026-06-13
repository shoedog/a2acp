//! Host-side prism diff-slice prep for implement-review (standard tier). The bridge runs the prism slicing
//! CLI on the committed diff and writes a defect-focused review slice to a reference file UNDER `.git/`.
//! Degrades to None on any failure — the slice is an accelerant, never a hard dependency.
use std::path::Path;
use std::time::Duration;

/// Injectable command runner (real = tokio process; tests = fake), so the seam is unit-tested without prism.
#[async_trait::async_trait]
pub trait SliceRunner: Send + Sync {
    /// Run `git diff base..head` then the prism CLI; return the slice text, or None on any failure/timeout.
    async fn produce(
        &self,
        clone: &Path,
        base: &str,
        head: &str,
        prism: &Path,
        timeout: Duration,
    ) -> Option<String>;
}

/// Write `text` (truncated head+tail to `max_bytes`) to `ref_path`, creating parent dirs. Best-effort.
pub fn write_slice(ref_path: &Path, text: &str, max_bytes: usize) -> std::io::Result<()> {
    if let Some(parent) = ref_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    const MARKER: &str = "\n…[slice truncated]…\n";
    let body = if text.len() > max_bytes {
        let half = max_bytes.saturating_sub(MARKER.len()) / 2;
        let head_end = (0..=half.min(text.len()))
            .rev()
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(0);
        let tail_start = {
            let raw = text.len().saturating_sub(half);
            (raw..=text.len())
                .find(|&i| text.is_char_boundary(i))
                .unwrap_or(text.len())
        };
        format!("{}{MARKER}{}", &text[..head_end], &text[tail_start..])
    } else {
        text.to_string()
    };
    std::fs::write(ref_path, body)
}

/// Production runner: `git diff base..head > tmp` in the clone, then `prism --repo <clone> --diff tmp
/// --format review`, bounded by `timeout`. Any nonzero/spawn/timeout → None (degrade).
pub struct ProdSliceRunner;

#[async_trait::async_trait]
impl SliceRunner for ProdSliceRunner {
    async fn produce(
        &self,
        clone: &Path,
        base: &str,
        head: &str,
        prism: &Path,
        timeout: Duration,
    ) -> Option<String> {
        tokio::time::timeout(timeout, async {
            // --no-ext-diff/--no-textconv: never invoke an external diff/textconv driver (could hang/be hostile).
            let diff = tokio::process::Command::new("git")
                .current_dir(clone)
                .args([
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    &format!("{base}..{head}"),
                ])
                .output()
                .await
                .ok()?;
            if !diff.status.success() {
                return None;
            }
            let tmp = clone.join(".git/a2a-bridge/review-slices/diff.patch");
            if let Some(p) = tmp.parent() {
                std::fs::create_dir_all(p).ok()?;
            }
            std::fs::write(&tmp, &diff.stdout).ok()?;
            let out = tokio::process::Command::new(prism)
                .current_dir(clone)
                .args(["--repo"])
                .arg(clone)
                .args(["--diff"])
                .arg(&tmp)
                .args(["--format", "review"])
                .output()
                .await
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
        })
        .await
        .ok()
        .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeOk;
    #[async_trait::async_trait]
    impl SliceRunner for FakeOk {
        async fn produce(
            &self,
            _c: &Path,
            _b: &str,
            _h: &str,
            _p: &Path,
            _t: Duration,
        ) -> Option<String> {
            Some("SLICE BODY".to_string())
        }
    }
    struct FakeNone;
    #[async_trait::async_trait]
    impl SliceRunner for FakeNone {
        async fn produce(
            &self,
            _c: &Path,
            _b: &str,
            _h: &str,
            _p: &Path,
            _t: Duration,
        ) -> Option<String> {
            None
        }
    }

    #[tokio::test]
    async fn ok_runner_yields_a_slice_written_to_the_ref_file() {
        let dir = std::env::temp_dir().join(format!("a2a-slice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let refp = dir.join(".git/a2a-bridge/review-slices/slice-1.md");
        let body = FakeOk
            .produce(
                &dir,
                "a",
                "b",
                Path::new("/x/prism"),
                Duration::from_secs(5),
            )
            .await;
        assert_eq!(body.as_deref(), Some("SLICE BODY"));
        write_slice(&refp, &body.unwrap(), 200_000).unwrap();
        assert_eq!(std::fs::read_to_string(&refp).unwrap(), "SLICE BODY");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn truncation_keeps_head_and_tail() {
        let dir = std::env::temp_dir().join(format!("a2a-slice-tr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let refp = dir.join("s.md");
        write_slice(&refp, &"x".repeat(1000), 100).unwrap();
        let got = std::fs::read_to_string(&refp).unwrap();
        // marker is "\n…[slice truncated]…\n" = 24 bytes; output must fit in max_bytes + marker.
        assert!(got.contains("slice truncated") && got.len() < 1000);
        assert!(
            got.len() <= 100 + 30,
            "truncated output exceeded honest cap: {} bytes",
            got.len()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn none_runner_degrades() {
        assert!(FakeNone
            .produce(
                Path::new("/c"),
                "a",
                "b",
                Path::new("/x"),
                Duration::from_secs(1)
            )
            .await
            .is_none());
    }

    #[tokio::test]
    async fn truncation_is_char_boundary_safe_on_multibyte() {
        let dir = std::env::temp_dir().join(format!("a2a-slice-mb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let refp = dir.join("s.md");
        // 1000 '…' chars (3 bytes each) — naive byte-slicing at an odd half would panic.
        let text = "…".repeat(1000);
        write_slice(&refp, &text, 101).unwrap(); // must not panic
        let got = std::fs::read_to_string(&refp).unwrap();
        assert!(got.contains("slice truncated"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
