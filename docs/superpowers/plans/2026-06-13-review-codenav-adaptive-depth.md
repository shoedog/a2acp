# Richer Review — Code-Nav Tooling + Adaptive Depth — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the bridge's reviews a consistent read-only code-nav toolset (prism nav + prism diff-slice → reference file + git archaeology) and add two-tier adaptive depth (light/standard) to `implement-review`, without diluting per-reviewer rigor.

**Architecture:** Pure helpers (`Tier`/`Depth`, `select_tier`, `parse_numstat`, `slice_ref_path`, slice-aware `build_review_input`) land in `bin/a2a-bridge/src/review.rs`; the impure orchestration (compute diff-stat → tier → run slice-prep → select workflow variant) extends `main.rs::run_review_step`; an injectable slice-prep seam runs the prism CLI. `[review]` config gains slice + threshold fields; the resume checkpoint stores a forced depth. Reviewer prompts gain a uniform contract; `containerized.toml` (+ podman twin) moves implement-review reviewers host-side with prism. Spec: `docs/superpowers/specs/2026-06-13-review-codenav-adaptive-depth-design.md`.

**Tech Stack:** Rust, tokio, `serde`, the workflow-DAG executor (`bridge-workflow`), the prism CLI (`~/code/slicing/target/release/prism`), `git`.

**Codebase note:** multi-crate workspace. `review.rs` is PURE (unit-tested); `main.rs::run_review_step` is the impure run (build-checked + live-gated). The implement loop runs `git reset --hard && git clean -fdq` each attempt and `--resume` refuses a dirty clone, so review artifacts MUST live under `<clone>/.git/a2a-bridge/` (where the checkpoint already lives).

---

### Task 1: `Tier` / `Depth` + `select_tier` (pure)  **[pure]**

**Files:**
- Modify: `bin/a2a-bridge/src/review.rs`
- Test: same file `#[cfg(test)]`

- [ ] **Step 1: Write the failing test** (append to the `tests` mod)

```rust
#[test]
fn select_tier_light_requires_both_under_thresholds() {
    // light iff lines <= light_max_lines AND files <= light_max_files; else standard.
    assert_eq!(select_tier(2, 10, 15, 2), Tier::Light);   // 2 files, 10 lines — both under
    assert_eq!(select_tier(2, 15, 15, 2), Tier::Light);   // boundary inclusive
    assert_eq!(select_tier(3, 10, 15, 2), Tier::Standard); // files over
    assert_eq!(select_tier(1, 16, 15, 2), Tier::Standard); // lines over
}

#[test]
fn resolve_depth_forced_overrides_auto() {
    assert_eq!(Depth::Forced(Tier::Standard).resolve(0, 0, 15, 2), Tier::Standard);
    assert_eq!(Depth::Auto.resolve(0, 0, 15, 2), Tier::Light);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge review::tests::select_tier_light_requires_both_under_thresholds`
Expected: FAIL (`Tier`/`select_tier` not found)

- [ ] **Step 3: Implement** (near the top of `review.rs`, after the `Verdict` enum)

```rust
/// Adaptive-depth tier. `thorough` is deferred (see the spec) — this slice has two tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Light,
    Standard,
}

/// Operator depth choice. `Auto` sizes from the diff each attempt; `Forced` pins a tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Auto,
    Forced(Tier),
}

impl Depth {
    /// Resolve to a concrete tier given this attempt's diff size + the config thresholds.
    pub fn resolve(self, files: usize, lines: usize, light_max_lines: usize, light_max_files: usize) -> Tier {
        match self {
            Depth::Forced(t) => t,
            Depth::Auto => select_tier(files, lines, light_max_lines, light_max_files),
        }
    }
}

/// PURE. light iff `lines <= light_max_lines` AND `files <= light_max_files`; else standard.
pub fn select_tier(files: usize, lines: usize, light_max_lines: usize, light_max_files: usize) -> Tier {
    if lines <= light_max_lines && files <= light_max_files {
        Tier::Light
    } else {
        Tier::Standard
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge review::tests::select_tier review::tests::resolve_depth`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/review.rs
git commit -m "feat(review): Tier/Depth + select_tier (pure)"
```

---

### Task 2: `parse_numstat` (pure)  **[pure]**

**Files:**
- Modify: `bin/a2a-bridge/src/review.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn parse_numstat_sums_added_deleted_and_counts_files() {
    // `git diff --numstat` lines: "<added>\t<deleted>\t<path>"; binary rows are "-\t-\t<path>".
    let out = "3\t1\tsrc/a.rs\n10\t0\tsrc/b.rs\n-\t-\tlogo.png\n";
    let (files, lines) = parse_numstat(out);
    assert_eq!(files, 3);          // 3 changed files incl. the binary
    assert_eq!(lines, 14);         // 3+1 + 10+0; binary contributes 0 lines
}

#[test]
fn parse_numstat_empty_is_zero() {
    assert_eq!(parse_numstat(""), (0, 0));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge review::tests::parse_numstat`
Expected: FAIL (`parse_numstat` not found)

- [ ] **Step 3: Implement**

```rust
/// PURE. Parse `git diff --numstat` → (changed_files, added+deleted_lines). A binary row (`-\t-\t<path>`)
/// counts toward files only. Malformed lines are skipped (the caller treats a total parse failure as standard).
pub fn parse_numstat(stdout: &str) -> (usize, usize) {
    let (mut files, mut lines) = (0usize, 0usize);
    for row in stdout.lines() {
        let mut cols = row.splitn(3, '\t');
        let (Some(a), Some(d), Some(_path)) = (cols.next(), cols.next(), cols.next()) else {
            continue;
        };
        files += 1;
        if let (Ok(added), Ok(deleted)) = (a.parse::<usize>(), d.parse::<usize>()) {
            lines += added + deleted;
        } // binary "-"/"-" → 0 lines
    }
    (files, lines)
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge review::tests::parse_numstat`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/review.rs
git commit -m "feat(review): parse_numstat (pure)"
```

---

### Task 3: `slice_ref_path` (pure, under `.git/`)  **[pure]**

**Files:**
- Modify: `bin/a2a-bridge/src/review.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn slice_ref_path_lives_under_git_a2a_bridge() {
    let p = slice_ref_path(std::path::Path::new("/clone"), "task-7-1");
    assert_eq!(
        p,
        std::path::PathBuf::from("/clone/.git/a2a-bridge/review-slices/slice-task-7-1.md")
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge review::tests::slice_ref_path_lives_under_git_a2a_bridge`
Expected: FAIL

- [ ] **Step 3: Implement** (add `use std::path::{Path, PathBuf};` at the top of `review.rs`)

```rust
/// PURE. The slice reference-file path, UNDER `.git/` so it survives `git reset --hard && git clean -fdq`,
/// never dirties the worktree, never blocks `--resume`, and is invisible to the write-capable fix turn.
pub fn slice_ref_path(clone: &Path, runid: &str) -> PathBuf {
    clone
        .join(".git/a2a-bridge/review-slices")
        .join(format!("slice-{runid}.md"))
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge review::tests::slice_ref_path_lives_under_git_a2a_bridge`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/review.rs
git commit -m "feat(review): slice_ref_path under .git/a2a-bridge"
```

---

### Task 4: slice-aware `build_review_input`  **[pure]**

**Files:**
- Modify: `bin/a2a-bridge/src/review.rs` (function + its existing test + the existing caller in `main.rs`)

- [ ] **Step 1: Update the existing test + add a slice case**

Replace `build_input_has_task_and_both_shas_and_diff` with:

```rust
#[test]
fn build_input_no_slice_has_task_shas_diff() {
    let i = build_review_input("do X", "aaa", "bbb", None);
    assert!(i.contains("do X") && i.contains("git diff aaa..bbb") && i.contains("DELIVER"));
    assert!(!i.contains("prism review-slice"));
}

#[test]
fn build_input_with_slice_points_at_the_ref_file() {
    let i = build_review_input("do X", "aaa", "bbb", Some(".git/a2a-bridge/review-slices/slice-1.md"));
    assert!(i.contains("prism review-slice") && i.contains("slice-1.md"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge review::tests::build_input`
Expected: FAIL (arity: `build_review_input` takes 3 args)

- [ ] **Step 3: Implement** — change the signature to take `slice_ref: Option<&str>`:

```rust
pub fn build_review_input(task: &str, base_sha: &str, head_sha: &str, slice_ref: Option<&str>) -> String {
    let slice = match slice_ref {
        Some(path) => format!(
            "\nA prism review-slice (defect-focused: blast radius, taint paths, missing symmetry) for this \
             diff is at `{path}` — read it FIRST as a map of where to look, then verify against the code.\n"
        ),
        None => String::new(),
    };
    format!(
        "TASK:\n{task}\n\n\
         Review the committed change in this repository: `git diff {base_sha}..{head_sha}`.\n\
         Use read-only git/grep/read to navigate the surrounding code. Assess: (1) does it DELIVER the \
         task (incl. implied requirements); (2) correctness/regressions/edge-cases; (3) design/architecture fit.{slice}"
    )
}
```

- [ ] **Step 4: Fix the caller** — `main.rs:945` currently `review::build_review_input(task, base_sha, head_sha)`. Change to pass `None` for now (Task 9 wires the real slice path):

```rust
let input = review::build_review_input(task, base_sha, head_sha, None);
```

- [ ] **Step 5: Run + build**

Run: `cargo test -p a2a-bridge review::tests::build_input && cargo build -p a2a-bridge`
Expected: PASS + builds

- [ ] **Step 6: Commit**

```bash
git add bin/a2a-bridge/src/review.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(review): build_review_input optionally references a slice file"
```

---

### Task 5: `[review]` config — slice + threshold fields  **[pure]**

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs` (`ReviewToml`, `ReviewConfig`, `to_config`)

**Read first:** `config.rs:479` `ReviewToml`, the `ReviewConfig` struct, and `ReviewToml::to_config` (search `fn to_config` near `ReviewToml`).

- [ ] **Step 1: Write the failing test** (in `config.rs` tests mod)

```rust
#[test]
fn review_toml_parses_slice_and_thresholds_with_defaults() {
    let t: ReviewToml = toml::from_str("workflow = \"implement-review\"").unwrap();
    let c = t.to_config().unwrap();
    assert_eq!(c.light_max_lines, 15);     // defaults
    assert_eq!(c.light_max_files, 2);
    assert!(c.slice_cmd.to_string_lossy().ends_with("prism"));
}

#[test]
fn review_toml_rejects_zero_thresholds() {
    let t: ReviewToml = toml::from_str("workflow=\"r\"\nlight_max_lines=0").unwrap();
    assert!(t.to_config().is_err());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge config::tests::review_toml_parses_slice`
Expected: FAIL (fields missing)

- [ ] **Step 3: Implement** — extend `ReviewToml` (defaults via helper fns) and `ReviewConfig`:

```rust
// in ReviewToml:
#[serde(default = "default_slice_cmd")]
pub slice_cmd: String,
#[serde(default = "default_slice_timeout_secs")]
pub slice_timeout_secs: u64,
#[serde(default = "default_slice_max_bytes")]
pub slice_max_bytes: usize,
#[serde(default = "default_light_max_lines")]
pub light_max_lines: usize,
#[serde(default = "default_light_max_files")]
pub light_max_files: usize,
```

```rust
fn default_slice_cmd() -> String { "~/code/slicing/target/release/prism".to_string() }
fn default_slice_timeout_secs() -> u64 { 60 }
fn default_slice_max_bytes() -> usize { 200_000 }
fn default_light_max_lines() -> usize { 15 }
fn default_light_max_files() -> usize { 2 }
```

```rust
// in ReviewConfig:
pub slice_cmd: std::path::PathBuf,        // ~ expanded
pub slice_timeout: std::time::Duration,
pub slice_max_bytes: usize,
pub light_max_lines: usize,
pub light_max_files: usize,
```

In `to_config`, after the existing fields, validate + expand `~`:

```rust
if self.light_max_lines == 0 || self.light_max_files == 0 {
    return Err(ConfigError::Registry("[review] light_max_lines/light_max_files must be > 0".into()));
}
let slice_cmd = std::path::PathBuf::from(
    shellexpand_tilde(&self.slice_cmd),  // see Step 3a
);
```

- [ ] **Step 3a: Tilde expansion** — add a tiny helper (no new dep; `$HOME`-based):

```rust
fn shellexpand_tilde(p: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(home) => format!("{home}/{rest}"),
            Err(_) => p.to_string(),
        },
        None => p.to_string(),
    }
}
```

(`ConfigError::Registry(String)` is the existing "invalid config value" variant `to_config` already uses for the workflow-id parse error.)

- [ ] **Step 4: Run + build**

Run: `cargo test -p a2a-bridge config::tests::review_toml && cargo build -p a2a-bridge`
Expected: PASS + builds (every `ReviewConfig { .. }` constructor now needs the new fields — update them; grep `ReviewConfig {`).

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(config): [review] slice_cmd + slice bounds + light thresholds (validated)"
```

---

### Task 6: slice-prep seam (injectable runner)  **[anchored]**

**Files:**
- Create: `bin/a2a-bridge/src/slice.rs`
- Modify: `bin/a2a-bridge/src/main.rs` (add `mod slice;`)

- [ ] **Step 1: Write the module with an injectable runner + a fake-runner test**

```rust
//! Host-side prism diff-slice prep for implement-review (standard tier). The bridge runs the prism slicing
//! CLI on the committed diff and writes a defect-focused review slice to a reference file UNDER `.git/`.
//! Degrades to None on any failure — the slice is an accelerant, never a hard dependency.
use std::path::Path;
use std::time::Duration;

/// Injectable command runner (real = tokio process; tests = fake), so the seam is unit-tested without prism.
#[async_trait::async_trait]
pub trait SliceRunner: Send + Sync {
    /// Run `git diff --base..head` then the prism CLI; return the slice text, or None on any failure/timeout.
    async fn produce(&self, clone: &Path, base: &str, head: &str, prism: &Path, timeout: Duration) -> Option<String>;
}

/// Write `text` (truncated head+tail to `max_bytes`) to `ref_path`, creating parent dirs. Best-effort.
pub fn write_slice(ref_path: &Path, text: &str, max_bytes: usize) -> std::io::Result<()> {
    if let Some(parent) = ref_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = if text.len() > max_bytes {
        let half = max_bytes / 2;
        format!("{}\n…[slice truncated]…\n{}", &text[..half], &text[text.len() - half..])
    } else {
        text.to_string()
    };
    std::fs::write(ref_path, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeOk;
    #[async_trait::async_trait]
    impl SliceRunner for FakeOk {
        async fn produce(&self, _c: &Path, _b: &str, _h: &str, _p: &Path, _t: Duration) -> Option<String> {
            Some("SLICE BODY".to_string())
        }
    }
    struct FakeNone;
    #[async_trait::async_trait]
    impl SliceRunner for FakeNone {
        async fn produce(&self, _c: &Path, _b: &str, _h: &str, _p: &Path, _t: Duration) -> Option<String> {
            None
        }
    }

    #[tokio::test]
    async fn ok_runner_yields_a_slice_written_to_the_ref_file() {
        let dir = std::env::temp_dir().join(format!("a2a-slice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let refp = dir.join(".git/a2a-bridge/review-slices/slice-1.md");
        let body = FakeOk.produce(&dir, "a", "b", Path::new("/x/prism"), Duration::from_secs(5)).await;
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
        assert!(got.contains("slice truncated") && got.len() < 1000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn none_runner_degrades() {
        assert!(FakeNone.produce(Path::new("/c"), "a", "b", Path::new("/x"), Duration::from_secs(1)).await.is_none());
    }
}
```

- [ ] **Step 2: Add `mod slice;`** beside the other `mod` lines in `main.rs` (after `mod review;`).

- [ ] **Step 3: Implement the production `SliceRunner`** in `slice.rs` (used by Task 9; not unit-tested — live-gated):

```rust
/// Production runner: `git diff base..head > tmp` in the clone, then `prism --repo <clone> --diff tmp
/// --format review`, bounded by `timeout`. Any nonzero/spawn/timeout → None (degrade).
pub struct ProdSliceRunner;

#[async_trait::async_trait]
impl SliceRunner for ProdSliceRunner {
    async fn produce(&self, clone: &Path, base: &str, head: &str, prism: &Path, timeout: Duration) -> Option<String> {
        let diff = tokio::process::Command::new("git")
            .current_dir(clone).args(["diff", &format!("{base}..{head}")])
            .output().await.ok()?;
        if !diff.status.success() { return None; }
        let tmp = clone.join(".git/a2a-bridge/review-slices/diff.patch");
        if let Some(p) = tmp.parent() { std::fs::create_dir_all(p).ok()?; }
        std::fs::write(&tmp, &diff.stdout).ok()?;
        let run = tokio::process::Command::new(prism)
            .current_dir(clone)
            .args(["--repo"]).arg(clone)
            .args(["--diff"]).arg(&tmp)
            .args(["--format", "review"])
            .output();
        let out = tokio::time::timeout(timeout, run).await.ok()?.ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }
}
```

- [ ] **Step 4: Run + build**

Run: `cargo test -p a2a-bridge slice::tests && cargo build -p a2a-bridge`
Expected: PASS + builds

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/slice.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(slice): injectable prism slice-prep seam (write under .git/, truncate, degrade)"
```

---

### Task 7: resume checkpoint — store forced depth  **[pure]**

**Files:**
- Modify: `bin/a2a-bridge/src/implement_resume.rs`
- Test: same file

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn checkpoint_round_trips_forced_depth_and_defaults_old() {
    // An older checkpoint JSON without the field deserializes with forced_depth = None.
    let old = r#"{"schema_version":1,"resume_id":"x","task_id":"x","task_brief":"b","source_repo":"/s","clone_path":"/c","config_path":"/cfg","branch":"br","base_ref":null,"base_commit":"abc","current_commit":null,"original_message":null,"edit_workflow":"e","fix_workflow":"f","loop_max_attempts":3,"attempt_next":1,"phase":"InLoop","created_at_ms":0,"updated_at_ms":0}"#;
    let cp: ImplementCheckpoint = serde_json::from_str(old).unwrap();
    assert_eq!(cp.forced_depth, None);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge implement_resume::tests::checkpoint_round_trips_forced_depth`
Expected: FAIL (no `forced_depth` field)

- [ ] **Step 3: Implement** — add to `ImplementCheckpoint` (after `attempt_next`):

```rust
/// Operator-forced review depth ("light"|"standard"), if any. `#[serde(default)]` so pre-existing
/// (schema-version-1) checkpoints read as None = auto-size each attempt.
#[serde(default)]
pub forced_depth: Option<String>,
```

(Every `ImplementCheckpoint { .. }` literal now needs `forced_depth`; grep and add `forced_depth: None` / the threaded value.)

- [ ] **Step 4: Run + build**

Run: `cargo test -p a2a-bridge implement_resume::tests && cargo build -p a2a-bridge`
Expected: PASS + builds

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/implement_resume.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(resume): checkpoint stores forced review depth (serde default = auto)"
```

---

### Task 8: `--depth` arg parsing  **[anchored]**

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (the implement arg parser + its struct)
- Test: in `main.rs` cli_tests

**Read first:** `parse_implement_args` (grep `fn parse_implement_args`) + its returned struct/flags.

- [ ] **Step 1: Write the failing test** (in `cli_tests`)

```rust
#[test]
fn parse_implement_depth_flag() {
    let a = super::parse_depth_flag(Some("light")).unwrap();
    assert_eq!(a, review::Depth::Forced(review::Tier::Light));
    assert_eq!(super::parse_depth_flag(None).unwrap(), review::Depth::Auto);
    assert!(super::parse_depth_flag(Some("thorough")).is_err()); // deferred → clear error
    assert!(super::parse_depth_flag(Some("bogus")).is_err());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge cli_tests::parse_implement_depth_flag`
Expected: FAIL

- [ ] **Step 3: Implement** — a pure helper + wire `--depth <v>` into `parse_implement_args` (mirror an existing `--flag value` arm) storing the raw string, then map via `parse_depth_flag`:

```rust
fn parse_depth_flag(v: Option<&str>) -> Result<review::Depth, BoxError> {
    match v {
        None => Ok(review::Depth::Auto),
        Some("light") => Ok(review::Depth::Forced(review::Tier::Light)),
        Some("standard") => Ok(review::Depth::Forced(review::Tier::Standard)),
        Some("thorough") => Err("--depth thorough is not yet supported (deferred); use light|standard".into()),
        Some(other) => Err(format!("--depth: unknown value {other:?} (expected light|standard)").into()),
    }
}
```

- [ ] **Step 4: Run + build**

Run: `cargo test -p a2a-bridge cli_tests::parse_implement_depth_flag && cargo build -p a2a-bridge`
Expected: PASS + builds

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(cli): implement --depth light|standard (thorough deferred)"
```

---

### Task 9: wire adaptive depth + slice into `run_review_step`  **[anchored]**

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (`run_review_step` + its callers: `ProdEffects::review`, the loop, resume)

**Read first:** `run_review_step` (`main.rs:920`), `ProdEffects` (`main.rs:989` — its `review` method calls `run_review_step`), and where `ProdEffects`/the loop is built so `depth` + the slice runner thread in.

- [ ] **Step 1: Add params + the tier/slice logic.** Give `run_review_step` two new params — `depth: review::Depth` and `slice: &dyn slice::SliceRunner` — and insert, after resolving `rcfg` and before `build_review_input`:

```rust
// Size the committed diff (auto recompute each attempt). git/parse failure → standard (safe).
let (files, lines) = match tokio::process::Command::new("git")
    .current_dir(std::path::Path::new(clone_cwd.as_str()))
    .args(["diff", "--numstat", &format!("{base_sha}..{head_sha}")])
    .output().await {
    Ok(o) if o.status.success() => review::parse_numstat(&String::from_utf8_lossy(&o.stdout)),
    _ => { eprintln!("[implement] review: numstat failed; defaulting to standard tier"); (usize::MAX, usize::MAX) }
};
let tier = depth.resolve(files, lines, rcfg.light_max_lines, rcfg.light_max_files);

// Slice prep ONLY for standard; write under .git/ (survives reset/clean). Degrade to None.
let runid = format!("{task_id}-{attempt}");
let slice_ref: Option<String> = if tier == review::Tier::Standard {
    let refp = review::slice_ref_path(std::path::Path::new(clone_cwd.as_str()), &runid);
    match slice.produce(std::path::Path::new(clone_cwd.as_str()), base_sha, head_sha, &rcfg.slice_cmd, rcfg.slice_timeout).await {
        Some(body) => match slice::write_slice(&refp, &body, rcfg.slice_max_bytes) {
            Ok(()) => Some(format!(".git/a2a-bridge/review-slices/slice-{runid}.md")),
            Err(e) => { eprintln!("[implement] review: slice write failed: {e}; continuing sliceless"); None }
        },
        None => { eprintln!("[implement] review: prism slice unavailable; continuing sliceless"); None }
    }
} else { None };

// Select the workflow variant by tier: standard → rcfg.workflow; light → <workflow>-light (fallback+warn).
let graph_id = match tier {
    review::Tier::Standard => rcfg.workflow.clone(),
    review::Tier::Light => {
        let light = bridge_core::ids::WorkflowId::parse(format!("{}-light", rcfg.workflow.as_str()));
        match light.ok().filter(|id| wf_map.contains_key(id)) {
            Some(id) => id,
            None => { eprintln!("[implement] review: no -light variant; falling back to standard workflow"); rcfg.workflow.clone() }
        }
    }
};
let Some(graph) = wf_map.get(&graph_id).cloned() else {
    return (review::ReviewOutcome::NotLoaded, String::new());
};
let input = review::build_review_input(task, base_sha, head_sha, slice_ref.as_deref());
```

Remove the now-replaced `let Some(graph) = wf_map.get(&rcfg.workflow)…` and `let input = build_review_input(.., None)` lines.

- [ ] **Step 2: Thread `depth` + a `ProdSliceRunner` through the callers.** `ProdEffects` gains a `depth: review::Depth` field (set from the parsed `--depth`, or from the checkpoint on resume) and passes `&slice::ProdSliceRunner` + `self.depth` into `run_review_step`. On resume, map `checkpoint.forced_depth` (`Some("light")`→Forced(Light), `Some("standard")`→Forced(Standard), `None`→Auto) and store the forced string into new checkpoints.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p a2a-bridge`
Expected: builds clean.

- [ ] **Step 4: Live check deferred to Task 12** (needs real prism + agents).

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(review): adaptive tier + slice-prep + variant selection in run_review_step"
```

---

### Task 10: prompts — uniform contract + prism for implement-review + light-synth  **[pure render]**

**Files:**
- Modify: the 11 reviewer prompts (§1 of the spec); Create: `prompts/implement-review-light-synth.md`
- Test: `bin/a2a-bridge/src/main.rs` cli_tests (prompt-contract assertions)

- [ ] **Step 1: Write the failing prompt-contract test**

```rust
#[test]
fn reviewer_prompts_carry_line_by_line_and_git_archaeology() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prompts");
    let reviewers = [
        "review-implement.md","review-correctness.md","review-architecture.md",
        "spec-review-rigor.md","spec-review-rigor-refine.md","spec-review-soundness.md",
        "spec-review-soundness-refine.md","plan-review-exec.md","plan-review-exec-refine.md",
        "plan-review-coverage.md","plan-review-coverage-refine.md",
    ];
    for f in reviewers {
        let t = std::fs::read_to_string(dir.join(f)).unwrap();
        assert!(t.to_lowercase().contains("line-by-line"), "{f}: missing line-by-line clause");
        assert!(t.contains("git blame") && t.contains("log -L"), "{f}: missing git archaeology");
    }
    // implement-review additionally gains the prism block.
    let ri = std::fs::read_to_string(dir.join("review-implement.md")).unwrap();
    assert!(ri.contains("prism") && ri.contains("nav_"), "review-implement missing prism block");
    // synth prompts are NOT required to carry the reviewer contract.
    let synth = std::fs::read_to_string(dir.join("implement-review-light-synth.md")).unwrap();
    assert!(synth.contains("{{reviewer}}") && !synth.contains("{{reviewer_claude}}"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge cli_tests::reviewer_prompts_carry_line_by_line`
Expected: FAIL

- [ ] **Step 3: Edit the prompts.** In each of the 11 reviewer prompts, inside the READ-ONLY contract block, add to the allowed read-only tools: ``git blame`, `git log -L <range>:<file>`, and `git log -S/-G` (pickaxe)``, and add the sentence: *"Do a thorough, human-style line-by-line reading and analysis of the artifact, regardless of its size — depth selection never licenses a shallower read."* In `review-implement.md` ONLY, also paste the prism block verbatim from `review-correctness.md` (the `**prism (if code-graph nav tools…)**` paragraph) and a line: *"If a `prism review-slice` path is named in the task input, read it first."*

- [ ] **Step 4: Create `prompts/implement-review-light-synth.md`** (single-reviewer verdict synth — copy `review-implement-synth.md` and collapse to one input):

```markdown
You are the synthesizer for a SINGLE-reviewer (light-tier) implement-review. Read the one reviewer's findings
and emit the final verdict for the committed change.

- Keep the reviewer's strongest, traced findings. A BLOCKER or a correctness MAJOR that means the change is
  wrong/unsound ⇒ REJECT. Otherwise APPROVE.
- End with EXACTLY this footer (and nothing after it but an optional `SUMMARY:` line):

VERDICT: APPROVE|REJECT
SUMMARY: <one line>

REVIEWER FINDINGS:
{{reviewer}}

(Change under review: {{input}})
```

- [ ] **Step 5: Run + build**

Run: `cargo test -p a2a-bridge cli_tests::reviewer_prompts_carry_line_by_line && cargo build -p a2a-bridge`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add prompts/ bin/a2a-bridge/src/main.rs
git commit -m "feat(prompts): uniform line-by-line + git-archaeology; prism for implement-review; light-synth"
```

---

### Task 11: example configs — host-side reviewers + variants + thresholds  **[anchored]**

**Files:**
- Modify: `examples/a2a-bridge.containerized.toml` AND `examples/a2a-bridge.containerized.podman.toml`
- Test: the existing `reference_containerized_config_parses_and_loads` + `podman_example…mirrors_docker` (Task makes them pass with the new content)

**Read first:** the `[[agents]] codex`/`claude` blocks (with `[agents.sandbox]`), the `[[workflows]] implement-review` block, and `[review]`/`allowed_cmds` in `containerized.toml`; the parity test `podman_example_parses_validates_and_mirrors_docker` (`main.rs`).

- [ ] **Step 1: Edit `containerized.toml`:**
  - The `codex` reviewer: remove its `[agents.sandbox]`; add `[[agents.mcp]]` prism (`command = "/Users/wesleyjinks/code/slicing/target/release/prism-mcp"`, `args = ["--repo","{cwd}","--cache-dir","/Users/wesleyjinks/.local/share/a2a/prism-cache-host"]`); add `-c sandbox_mode="read-only"` to its `args`.
  - The `claude` reviewer: remove its `[agents.sandbox]`; add the same `[[agents.mcp]]` prism block.
  - `allowed_cmds`: add `"codex-acp"` and `"claude-agent-acp"` (keep `"docker"` for `impl`).
  - Add a `[[workflows]] implement-review-light` (1 reviewer node + a synth node using `prompt_file = "../prompts/implement-review-light-synth.md"`, `inputs = ["<reviewer>"]`, terminal).
  - Add `[review]` thresholds: `light_max_lines`, `light_max_files`, `slice_cmd` (if non-default).
- [ ] **Step 2: Mirror EVERY structural change into `containerized.podman.toml`** (the parity test strips only comments/`runtime`/`allowed_cmds` lines — model/mcp/workflow lines must match byte-for-byte).
- [ ] **Step 3: Verify structural parity + parse**

Run:
```bash
cargo test -p a2a-bridge reference_containerized_config_parses_and_loads podman_example_parses_validates_and_mirrors_docker
```
Expected: PASS (both)

- [ ] **Step 4: Commit**

```bash
git add examples/a2a-bridge.containerized.toml examples/a2a-bridge.containerized.podman.toml
git commit -m "feat(examples): host-side implement-review reviewers + prism + implement-review-light + [review] thresholds"
```

---

### Task 12: full gate, docs, live DoD

**Files:**
- Modify: `docs/onboarding.md` (the review section), `AGENTS.md` (`implement --depth`)

- [ ] **Step 1: Pre-merge gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p a2a-bridge`
Expected: clean. (CI also runs `cargo deny` + coverage floors — keep them green.)

- [ ] **Step 2: Docs** — in `docs/onboarding.md` note adaptive depth (`light`/`standard`, auto from diff size, `--depth` override) + the code-nav toolset (prism nav + slice reference file + git archaeology). In `AGENTS.md`, add `--depth light|standard` to the `implement` line.

- [ ] **Step 3: Live DoD** (real prism + host agents; containers idle):
  - A tiny-diff `implement` → **light** (1 lens, no slice; verdict via `implement-review-light-synth`).
  - A normal-diff `implement` → **standard** (2 lenses + a slice ref present under `<clone>/.git/a2a-bridge/review-slices/` and referenced in the prompt); confirm the reviewers call `mcp__prism__nav_*` (inspect `docker logs` / the bridge's `agent_stderr`) and read the slice file.
  - Degrade: rename `slice_cmd` to a bad path → review still completes + verdicts.
  - Crash mid-review then `implement --resume <id>` → not refused (slice is under `.git/`).
  - Dogfood THIS plan + spec through the bridge's own spec/plan reviews.

- [ ] **Step 4: Commit**

```bash
git add docs/onboarding.md AGENTS.md
git commit -m "docs: adaptive review depth + code-nav toolset"
```

---

## Self-review notes (resolved)

- **Spec coverage:** uniform contract (T10), prism nav consistency + host-side wiring (T10/T11), slice→ref-file under `.git/` (T3,T4,T6,T9), git archaeology (T10), two-tier adaptive depth (T1,T8,T9), select_tier/numstat/thresholds (T1,T2,T5), variant selection + fallback (T9), depth×resume (T7,T9), error/degrade (T6,T9), tests + live DoD (every task + T12). Deferred items (thorough, code-review slice, depth-gate, LSP) are out of scope by construction.
- **Type consistency:** `Tier`/`Depth` (T1) used in T8/T9; `parse_numstat` (T2)→T9; `slice_ref_path` (T3)→T9; `build_review_input(.., Option<&str>)` (T4) caller fixed same task, real slice in T9; `ReviewConfig` new fields (T5) read in T9; `SliceRunner`/`write_slice` (T6) used in T9; `forced_depth` (T7) mapped in T9.
- **Anchored tasks (read-then-edit):** T6/T8/T9/T11 touch large files; each cites the read anchor. T9 is build-checked + live-gated (the pure pieces it composes are unit-tested in T1–T6).
