# Thorough Review-Depth Tier Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a third `implement`-review depth tier (`thorough` = draft→refine→synth) and rework diff-sizing to count LLOC over code/infra files only, with a deterministic owner/caller/constructor depth-override seam.

**Architecture:** All sizing/tier/verdict logic stays in the pure `bin/a2a-bridge/src/review.rs` module (unit-tested); orchestration stays in `main.rs::run_review_step`. The thorough workflow + refine prompt are config/prompt-only (example-only, mirroring how `light` ships). Tasks are ordered to keep `cargo build` + `clippy -D warnings` green at every commit.

**Tech Stack:** Rust (edition 2021, toolchain 1.94.0), tokio, the in-repo `bridge-workflow` executor. Spec: `docs/superpowers/specs/2026-06-13-thorough-review-tier-design.md`.

---

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `bin/a2a-bridge/src/review.rs` | PURE tier/sizing/verdict/reduction | `Tier::Thorough`, 3-way `select_tier`, `tier_workflow_suffix`, `Depth::resolve(Option)`, relocated `Depth::{parse_flag,from_forced_str,to_forced_str}`, `counts_toward_depth`, path-aware `is_logical_line`, hunk-stateful `parse_diff_for_depth` (replaces `parse_numstat`), per-leg `reduce()` |
| `bin/a2a-bridge/src/config.rs` | `[review]` parse + validation | `thorough_min_lines`/`thorough_min_files`/`default_depth` + ordered-band validation |
| `bin/a2a-bridge/src/main.rs` | `implement` orchestration | `run_review_step` sizing/variant, `variant_or_fallback`, `Option<Depth>` seam, resume precedence, usage strings; delete local depth helpers |
| `bin/a2a-bridge/src/implement_resume.rs` | resume checkpoint | no struct change; resume path consumes `Option<Depth>` |
| `prompts/review-implement-refine.md` | NEW thorough refine prompt | create |
| `examples/a2a-bridge.containerized.toml` + `.podman.toml` | dogfood config | `implement-review-thorough` workflow + `[review]` fields + timeout 1800 (mirrored) |

**Conventions for every task:** run `cargo test -p a2a-bridge <name>` to scope unit tests; the full gate is `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`. Use `cargo fmt --all` before each commit. Commit messages use `feat(review):` / `refactor(review):` and end with the Co-Authored-By trailer.

---

### Task 1: Tier ladder + config thresholds + tier resolution wiring

Adds `Tier::Thorough` and the 3-way selection. Because the new enum variant makes `run_review_step`'s `match tier` non-exhaustive and `select_tier`/`Depth::resolve` change signature, this task lands the config thresholds, the pure changes, AND the `run_review_step` wiring together so the crate stays green. Sizing still uses `parse_numstat` (wrapped in `Option`); the patch-parser swap is Task 2. This task also fixes the auto-failure→Thorough bug by making sizing an `Option` (`None` → Standard).

**Files:**
- Modify: `bin/a2a-bridge/src/review.rs` (Tier, select_tier, resolve, tier_workflow_suffix)
- Modify: `bin/a2a-bridge/src/config.rs:499-573` (ReviewToml/ReviewConfig/to_config)
- Modify: `bin/a2a-bridge/src/main.rs:1024,1028,1056-1070` (run_review_step) + add `variant_or_fallback`

- [ ] **Step 1: Write failing review.rs tests for the 3-way tier + Option resolve**

Replace the existing `select_tier_light_requires_both_under_thresholds` and `resolve_depth_forced_overrides_auto` tests in `review.rs` (the old 4-arg signatures) with:

```rust
    #[test]
    fn select_tier_three_way() {
        // signature: (files, lines, light_max_lines, light_max_files, thorough_min_lines, thorough_min_files)
        // light: both under
        assert_eq!(select_tier(2, 15, 15, 2, 150, 6), Tier::Light);
        // standard: in the band
        assert_eq!(select_tier(3, 16, 15, 2, 150, 6), Tier::Standard);
        assert_eq!(select_tier(5, 149, 15, 2, 150, 6), Tier::Standard);
        // thorough by lines OR files
        assert_eq!(select_tier(1, 150, 15, 2, 150, 6), Tier::Thorough);
        assert_eq!(select_tier(6, 10, 15, 2, 150, 6), Tier::Thorough);
    }

    #[test]
    fn resolve_forced_wins_and_unknown_is_standard() {
        // Forced always wins, even with no sizing.
        assert_eq!(Depth::Forced(Tier::Thorough).resolve(None, 15, 2, 150, 6), Tier::Thorough);
        assert_eq!(Depth::Forced(Tier::Light).resolve(Some((9, 9)), 15, 2, 150, 6), Tier::Light);
        // Auto + Some sizes; Auto + None (git/parse failure) → Standard fail-safe (NOT Thorough).
        assert_eq!(Depth::Auto.resolve(Some((0, 0)), 15, 2, 150, 6), Tier::Light);
        assert_eq!(Depth::Auto.resolve(None, 15, 2, 150, 6), Tier::Standard);
    }

    #[test]
    fn tier_workflow_suffix_maps_all_tiers() {
        assert_eq!(tier_workflow_suffix(Tier::Light), "-light");
        assert_eq!(tier_workflow_suffix(Tier::Standard), "");
        assert_eq!(tier_workflow_suffix(Tier::Thorough), "-thorough");
    }
```

- [ ] **Step 2: Run the tests — verify they fail to compile**

Run: `cargo test -p a2a-bridge select_tier_three_way 2>&1 | head`
Expected: compile error — `Tier::Thorough` undefined, `select_tier` arity mismatch, `resolve` takes `Option`.

- [ ] **Step 3: Implement the review.rs pure changes**

In `review.rs`, change the `Tier` enum and the doc comment:

```rust
/// Adaptive-depth tier. `thorough` = draft→refine double pass for large code/infra diffs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Light,
    Standard,
    Thorough,
}
```

Replace `Depth::resolve` and `select_tier` with:

```rust
impl Depth {
    /// Resolve to a concrete tier. `Forced` always wins; `Auto` sizes from `sizing` when known, else
    /// (git/parse failure) falls back to `Standard` — NEVER `Thorough` (the unknown-size fail-safe).
    /// `sizing` is `(files, lines)` over code/infra only.
    pub fn resolve(
        self,
        sizing: Option<(usize, usize)>,
        light_max_lines: usize,
        light_max_files: usize,
        thorough_min_lines: usize,
        thorough_min_files: usize,
    ) -> Tier {
        match (self, sizing) {
            (Depth::Forced(t), _) => t,
            (Depth::Auto, Some((files, lines))) => select_tier(
                files,
                lines,
                light_max_lines,
                light_max_files,
                thorough_min_lines,
                thorough_min_files,
            ),
            (Depth::Auto, None) => Tier::Standard,
        }
    }
}

/// PURE. light iff `files ≤ light_max_files` AND `lines ≤ light_max_lines`; else thorough iff
/// `lines ≥ thorough_min_lines` OR `files ≥ thorough_min_files`; else standard. Light is checked first;
/// config validation guarantees `thorough_min_* > light_max_*` so the bands cannot overlap.
pub fn select_tier(
    files: usize,
    lines: usize,
    light_max_lines: usize,
    light_max_files: usize,
    thorough_min_lines: usize,
    thorough_min_files: usize,
) -> Tier {
    if lines <= light_max_lines && files <= light_max_files {
        Tier::Light
    } else if lines >= thorough_min_lines || files >= thorough_min_files {
        Tier::Thorough
    } else {
        Tier::Standard
    }
}

/// PURE. The workflow-id suffix for a tier (`Standard` = the base workflow, no suffix).
pub fn tier_workflow_suffix(tier: Tier) -> &'static str {
    match tier {
        Tier::Light => "-light",
        Tier::Standard => "",
        Tier::Thorough => "-thorough",
    }
}
```

- [ ] **Step 4: Add the config thresholds**

In `config.rs`, after `default_light_max_files` (line ~492) add:

```rust
fn default_thorough_min_lines() -> usize {
    150
}

fn default_thorough_min_files() -> usize {
    6
}
```

In `struct ReviewToml` (after `light_max_files`, line ~515) add:

```rust
    #[serde(default = "default_thorough_min_lines")]
    pub thorough_min_lines: usize,
    #[serde(default = "default_thorough_min_files")]
    pub thorough_min_files: usize,
```

In `struct ReviewConfig` (after `light_max_files`, line ~529) add:

```rust
    pub thorough_min_lines: usize,
    pub thorough_min_files: usize,
```

In `ReviewToml::to_config`, replace the `light_max_*` zero-check block (lines ~551-555) with the ordered-band validation, and add the two fields to the returned struct:

```rust
        if self.light_max_lines == 0 || self.light_max_files == 0 {
            return Err(ConfigError::Registry(
                "[review] light_max_lines/light_max_files must be > 0".into(),
            ));
        }
        if self.thorough_min_lines <= self.light_max_lines
            || self.thorough_min_files <= self.light_max_files
        {
            return Err(ConfigError::Registry(
                "[review] thorough_min_lines/thorough_min_files must be > light_max_lines/light_max_files".into(),
            ));
        }
```

And in the `Ok(ReviewConfig { ... })` literal (after `light_max_files: self.light_max_files,`):

```rust
            thorough_min_lines: self.thorough_min_lines,
            thorough_min_files: self.thorough_min_files,
```

- [ ] **Step 5: Wire run_review_step (sizing → Option, slice, variant)**

In `main.rs`, replace the numstat block + tier line (lines 999-1024) — keep `--numstat` for now (Task 2 swaps to the patch), but make sizing an `Option` and pass the new thresholds:

```rust
    let sizing: Option<(usize, usize)> = match tokio::time::timeout(
        rcfg.slice_timeout,
        tokio::process::Command::new("git")
            .current_dir(clone_path)
            .args([
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--numstat",
                &format!("{base_sha}..{head_sha}"),
            ])
            .output(),
    )
    .await
    {
        Ok(Ok(o)) if o.status.success() => {
            Some(review::parse_numstat(&String::from_utf8_lossy(&o.stdout)))
        }
        _ => {
            eprintln!("[implement] review: numstat failed or timed out; sizing unknown → standard tier");
            None
        }
    };
    let tier = depth.resolve(
        sizing,
        rcfg.light_max_lines,
        rcfg.light_max_files,
        rcfg.thorough_min_lines,
        rcfg.thorough_min_files,
    );
```

Change the slice condition (line 1028) from `if tier == review::Tier::Standard {` to:

```rust
    let slice_ref: Option<String> = if tier != review::Tier::Light {
```

Replace the `graph_id` match (lines 1056-1070) with:

```rust
    // Select the workflow variant by tier: standard → base; light/thorough → <base>-<suffix> (fallback+warn).
    let graph_id = match tier {
        review::Tier::Standard => rcfg.workflow.clone(),
        review::Tier::Light => variant_or_fallback(wf_map, &rcfg.workflow, "-light"),
        review::Tier::Thorough => variant_or_fallback(wf_map, &rcfg.workflow, "-thorough"),
    };
```

Add the helper just above `async fn run_review_step` (line ~967):

```rust
/// Resolve `<base><suffix>` from the loaded workflows; warn + fall back to the base (standard) workflow
/// when the variant is absent. Slice presence still follows the TIER, not the resolved workflow.
fn variant_or_fallback(
    wf_map: &std::collections::HashMap<
        bridge_core::ids::WorkflowId,
        std::sync::Arc<bridge_workflow::graph::WorkflowGraph>,
    >,
    base: &bridge_core::ids::WorkflowId,
    suffix: &str,
) -> bridge_core::ids::WorkflowId {
    match bridge_core::ids::WorkflowId::parse(format!("{}{}", base.as_str(), suffix))
        .ok()
        .filter(|id| wf_map.contains_key(id))
    {
        Some(id) => id,
        None => {
            eprintln!(
                "[implement] review: no {}{} variant; falling back to standard workflow",
                base.as_str(),
                suffix
            );
            base.clone()
        }
    }
}
```

- [ ] **Step 6: Update the two config tests that assert review defaults**

In `config.rs`, extend `review_toml_parses_slice_and_thresholds_with_defaults` (line ~2110) with:

```rust
        assert_eq!(c.thorough_min_lines, 150);
        assert_eq!(c.thorough_min_files, 6);
```

Add a new test next to it:

```rust
    #[test]
    fn review_toml_rejects_unordered_bands() {
        let t: ReviewToml =
            toml::from_str("workflow=\"r\"\nlight_max_lines=200\nthorough_min_lines=100").unwrap();
        assert!(t.to_config().is_err());
    }
```

- [ ] **Step 7: Run the gate**

Run: `cargo test -p a2a-bridge select_tier_three_way resolve_forced tier_workflow_suffix review_toml_ && cargo clippy -p a2a-bridge --all-targets -- -D warnings`
Expected: PASS; no warnings.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add bin/a2a-bridge/src/review.rs bin/a2a-bridge/src/config.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(review): add Thorough tier + 3-way select_tier + ordered-band thresholds; Option sizing fixes auto-failure fail-safe"
```

---

### Task 2: Diff sizing — hunk-stateful, side-aware `parse_diff_for_depth`

Replaces `parse_numstat` with a unified-diff parser that counts LLOC over code/infra files only. Adds `counts_toward_depth` + path-aware `is_logical_line`. Swaps the git invocation in `run_review_step` to the full patch.

**Files:**
- Modify: `bin/a2a-bridge/src/review.rs` (remove `parse_numstat` + its 2 tests; add 3 fns + helpers + tests)
- Modify: `bin/a2a-bridge/src/main.rs` (run_review_step git args + parse call)

- [ ] **Step 1: Write failing tests for the classifiers + parser**

In `review.rs`, delete the two `parse_numstat_*` tests and add:

```rust
    #[test]
    fn counts_toward_depth_excludes_docs_tests_locks_generated() {
        assert!(counts_toward_depth("src/main.rs"));
        assert!(counts_toward_depth("deploy/Containerfile"));
        assert!(counts_toward_depth("config/app.toml"));
        // ambiguous dirs still count (bias toward review)
        assert!(counts_toward_depth("src/build/mod.rs"));
        assert!(counts_toward_depth("src/gen/codegen.rs"));
        // excluded
        assert!(!counts_toward_depth("docs/x.md"));
        assert!(!counts_toward_depth("README.markdown"));
        assert!(!counts_toward_depth("tests/it.rs"));
        assert!(!counts_toward_depth("src/foo_test.go"));
        assert!(!counts_toward_depth("pkg/api.test.ts"));
        assert!(!counts_toward_depth("Cargo.lock"));
        assert!(!counts_toward_depth("frontend/yarn.lock"));
        assert!(!counts_toward_depth("go.sum"));
        assert!(!counts_toward_depth("web/app.min.js"));
        assert!(!counts_toward_depth("api/user.pb.go"));
        assert!(!counts_toward_depth("node_modules/x/index.js"));
        assert!(!counts_toward_depth("vendor/lib/x.go"));
    }

    #[test]
    fn is_logical_line_is_path_aware() {
        // Rust: attributes are CODE (not comments); // is a comment.
        assert!(is_logical_line("src/a.rs", "#[derive(Debug)]"));
        assert!(is_logical_line("src/a.rs", "#![allow(dead_code)]"));
        assert!(!is_logical_line("src/a.rs", "// a comment"));
        assert!(is_logical_line("src/a.rs", "    *ptr = x;")); // leading * deref counts
        // toml/yaml/sh/Dockerfile: # is a comment.
        assert!(!is_logical_line("a.toml", "# c"));
        assert!(!is_logical_line("Dockerfile", "# syntax=docker"));
        assert!(is_logical_line("a.toml", "key = 1"));
        // sql: -- is a comment but counts in .rs
        assert!(!is_logical_line("q.sql", "-- comment"));
        assert!(is_logical_line("src/a.rs", "x -= 1;"));
        // blanks excluded everywhere; unknown ext strips nothing.
        assert!(!is_logical_line("src/a.rs", "   "));
        assert!(is_logical_line("data.bin", "# not stripped for unknown ext"));
    }

    #[test]
    fn parse_diff_for_depth_counts_code_infra_lloc_only() {
        let patch = "\
diff --git a/src/a.rs b/src/a.rs
index 1..2 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,4 @@
 fn x() {}
+#[derive(Debug)]
+struct B;
+// just a comment
+
diff --git a/docs/readme.md b/docs/readme.md
--- a/docs/readme.md
+++ b/docs/readme.md
@@ -1 +1,9 @@
+lots of docs lines that must NOT count
diff --git a/Cargo.lock b/Cargo.lock
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -1 +1,40 @@
+churn churn
";
        // a.rs: #[derive] + struct B count (2); comment + blank excluded. docs + lock excluded.
        assert_eq!(parse_diff_for_depth(patch), (1, 2));
    }

    #[test]
    fn parse_diff_for_depth_hunk_stateful_does_not_misread_content() {
        // A removed YAML `---` renders as `----`; an added `+++` as `++++`; must count as changed lines,
        // not be misread as file headers.
        let patch = "\
diff --git a/k8s/x.yaml b/k8s/x.yaml
--- a/k8s/x.yaml
+++ b/k8s/x.yaml
@@ -1,2 +1,2 @@
-name: a
+++++name: b
";
        // one removed `-name: a` (logical) + one added `+++++name: b` (logical) = 2 lines, 1 file.
        assert_eq!(parse_diff_for_depth(patch), (1, 2));
    }

    #[test]
    fn parse_diff_for_depth_rename_out_of_code_counts_removals() {
        // src/a.rs renamed to docs/a.md WITH an edit: the new path is excluded (docs), but the old path
        // is code, so removed lines count; the added doc line does not.
        let patch = "\
diff --git a/src/a.rs b/docs/a.md
similarity index 60%
rename from src/a.rs
rename to docs/a.md
--- a/src/a.rs
+++ b/docs/a.md
@@ -1,2 +1,1 @@
-fn gone() {}
-let removed = 1;
+now docs
";
        assert_eq!(parse_diff_for_depth(patch), (1, 2));
    }

    #[test]
    fn parse_diff_for_depth_pure_rename_and_binary_are_zero() {
        let patch = "\
diff --git a/src/a.rs b/src/b.rs
similarity index 100%
rename from src/a.rs
rename to src/b.rs
diff --git a/logo.png b/logo.png
index 1..2 100644
Binary files a/logo.png and b/logo.png differ
";
        assert_eq!(parse_diff_for_depth(patch), (0, 0));
    }

    #[test]
    fn parse_diff_for_depth_quoted_md_path_excluded() {
        let patch = "\
diff --git \"a/docs/a b.md\" \"b/docs/a b.md\"
--- \"a/docs/a b.md\"
+++ \"b/docs/a b.md\"
@@ -1 +1,2 @@
+a doc line
";
        assert_eq!(parse_diff_for_depth(patch), (0, 0));
    }
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test -p a2a-bridge parse_diff_for_depth counts_toward_depth is_logical_line 2>&1 | head`
Expected: compile error — functions undefined.

- [ ] **Step 3: Implement the classifiers**

In `review.rs`, add (replacing the old `parse_numstat` fn):

```rust
/// PURE. Does this path count toward review-depth sizing? Markdown, tests, lockfiles, and generated files
/// are excluded so depth reflects the CODE+INFRA change. Ambiguous paths COUNT (bias toward review).
pub fn counts_toward_depth(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    if lower.ends_with(".md") || lower.ends_with(".markdown") {
        return false;
    }
    const LOCKFILES: &[&str] = &[
        "cargo.lock",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "gemfile.lock",
        "poetry.lock",
        "pipfile.lock",
        "composer.lock",
        "go.sum",
    ];
    if LOCKFILES.contains(&file) || lower.ends_with(".lock") {
        return false;
    }
    if lower.ends_with(".min.js")
        || lower.ends_with(".min.css")
        || lower.ends_with(".pb.go")
        || lower.ends_with(".snap")
        || lower.ends_with(".g.dart")
        || lower.contains("_pb2.")
        || lower.contains(".generated.")
        || lower.contains("_generated.")
    {
        return false;
    }
    const TEST_DIRS: &[&str] = &["tests", "test", "__tests__", "__mocks__", "testdata", "fixtures"];
    const GEN_DIRS: &[&str] = &["node_modules", "vendor", "dist", "target", ".next"];
    if lower
        .split('/')
        .any(|c| TEST_DIRS.contains(&c) || GEN_DIRS.contains(&c))
    {
        return false;
    }
    if file.contains("_test.")
        || file.contains("_tests.")
        || file.starts_with("test_")
        || file.contains(".test.")
        || file.contains(".spec.")
        || file.contains("_spec.")
    {
        return false;
    }
    true
}

/// PURE. Comment markers for a path's language (line-comment + block-open). Empty = strip nothing.
fn comment_markers(path: &str) -> &'static [&'static str] {
    let lower = path.to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    if file == "dockerfile" || file.ends_with(".dockerfile") {
        return &["#"];
    }
    let ext = file.rsplit('.').next().unwrap_or("");
    match ext {
        "rs" => &["//"], // NOT `#` — `#[derive]`/`#![…]` attributes are CODE
        "toml" | "yaml" | "yml" | "sh" | "bash" | "py" | "rb" | "cfg" | "ini" | "conf" => &["#"],
        "c" | "h" | "cpp" | "hpp" | "cc" | "go" | "js" | "jsx" | "ts" | "tsx" | "java" | "kt"
        | "swift" => &["//", "/*"],
        "sql" | "lua" | "hs" => &["--"],
        "html" | "xml" | "vue" | "svelte" => &["<!--"],
        _ => &[],
    }
}

/// PURE, path-aware. A changed line is "logical" unless blank or a leading comment for that language.
/// Approximate LLOC (block-comment interiors and string-embedded markers are known, conservative limits).
pub fn is_logical_line(path: &str, content: &str) -> bool {
    let t = content.trim_start();
    if t.is_empty() {
        return false;
    }
    !comment_markers(path).iter().any(|m| t.starts_with(m))
}
```

- [ ] **Step 4: Implement `parse_diff_for_depth` + path helpers**

```rust
/// PURE. Best-effort unquote of a Git C-quoted path (`"a/x\ty"`). With `core.quotePath=false` the only
/// remaining escapes are control chars / quotes; anything undecoded falls through (fail-open → counts).
fn unquote_path(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('t') => out.push('\t'),
                    Some('n') => out.push('\n'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(other) => out.push(other),
                    None => {}
                }
            } else {
                out.push(c);
            }
        }
        out
    } else {
        s.to_string()
    }
}

/// PURE. The path from a `--- <s>` / `+++ <s>` header: unquote, drop the `a/`/`b/` prefix, `/dev/null`→None.
fn header_path(s: &str, side: char) -> Option<String> {
    let s = s.trim();
    if s == "/dev/null" {
        return None;
    }
    let unq = unquote_path(s);
    let prefix = format!("{side}/");
    Some(unq.strip_prefix(&prefix).unwrap_or(&unq).to_string())
}

/// PURE, HUNK-STATEFUL + side-aware. Parse a unified-diff patch → (code_infra_files, lloc). File paths
/// come from the header phase; lines are classified ONLY inside hunk phase by their leading byte. A `+`
/// line is gated by the NEW path, a `-` line by the OLD path; a file counts iff it has ≥1 logical line.
pub fn parse_diff_for_depth(patch: &str) -> (usize, usize) {
    let mut files = 0usize;
    let mut lloc = 0usize;
    let mut old_path: Option<String> = None;
    let mut new_path: Option<String> = None;
    let mut file_logical = 0usize;
    let mut in_hunk = false;

    for raw in patch.lines() {
        if raw.starts_with("diff --git ") {
            if file_logical > 0 {
                files += 1;
                lloc += file_logical;
            }
            old_path = None;
            new_path = None;
            file_logical = 0;
            in_hunk = false;
            continue;
        }
        if !in_hunk {
            if let Some(p) = raw.strip_prefix("--- ") {
                old_path = header_path(p, 'a');
            } else if let Some(p) = raw.strip_prefix("+++ ") {
                new_path = header_path(p, 'b');
            } else if let Some(p) = raw.strip_prefix("rename from ") {
                old_path = Some(unquote_path(p));
            } else if let Some(p) = raw.strip_prefix("rename to ") {
                new_path = Some(unquote_path(p));
            } else if let Some(p) = raw.strip_prefix("copy from ") {
                old_path = Some(unquote_path(p));
            } else if let Some(p) = raw.strip_prefix("copy to ") {
                new_path = Some(unquote_path(p));
            } else if raw.starts_with("@@") {
                in_hunk = true;
            }
            continue;
        }
        match raw.as_bytes().first().copied() {
            None => {}                                  // empty line: ignore, stay in hunk
            Some(b'@') if raw.starts_with("@@") => {}   // next hunk header
            Some(b'\\') => {}                           // "\ No newline at end of file"
            Some(b'+') => {
                let p = new_path.as_deref().unwrap_or("");
                if counts_toward_depth(p) && is_logical_line(p, &raw[1..]) {
                    file_logical += 1;
                }
            }
            Some(b'-') => {
                let p = old_path.as_deref().unwrap_or("");
                if counts_toward_depth(p) && is_logical_line(p, &raw[1..]) {
                    file_logical += 1;
                }
            }
            Some(b' ') => {}                            // context
            Some(_) => in_hunk = false,                 // non-marker line ends the hunk body
        }
    }
    if file_logical > 0 {
        files += 1;
        lloc += file_logical;
    }
    (files, lloc)
}
```

- [ ] **Step 5: Run the new tests**

Run: `cargo test -p a2a-bridge parse_diff_for_depth counts_toward_depth is_logical_line`
Expected: PASS (all 7 tests).

- [ ] **Step 6: Swap the git invocation in `run_review_step` to the full patch**

In `main.rs::run_review_step`, change the git args (add `-c core.quotePath=false`, drop `--numstat`) and the parse call:

```rust
        tokio::process::Command::new("git")
            .current_dir(clone_path)
            .args([
                "-c",
                "core.quotePath=false",
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                &format!("{base_sha}..{head_sha}"),
            ])
            .output(),
```

and:

```rust
        Ok(Ok(o)) if o.status.success() => {
            Some(review::parse_diff_for_depth(&String::from_utf8_lossy(&o.stdout)))
        }
```

Update the `eprintln!` text from "numstat failed" to "diff sizing failed":

```rust
            eprintln!("[implement] review: diff sizing failed or timed out; sizing unknown → standard tier");
```

- [ ] **Step 7: Run the gate**

Run: `cargo test -p a2a-bridge && cargo clippy -p a2a-bridge --all-targets -- -D warnings`
Expected: PASS; no warnings (confirms `parse_numstat` has no remaining callers).

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add bin/a2a-bridge/src/review.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(review): hunk-stateful side-aware parse_diff_for_depth (LLOC over code/infra); replace parse_numstat"
```

---

### Task 3: Per-leg `reduce()` failed-reviewer accounting

The executor runs a refine node even when its draft input failed (`executor.rs:284` schedules on `done`, not `ok`). Count distinct logical legs (strip a `_draft` suffix, dedup) so a collapsed reviewer counts once across light/standard/thorough.

**Files:**
- Modify: `bin/a2a-bridge/src/review.rs` (`reduce` + tests)

- [ ] **Step 1: Write the failing test**

Add to `review.rs` tests:

```rust
    #[test]
    fn reduce_dedups_draft_and_refine_into_one_leg() {
        use bridge_core::ids::NodeId;
        use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome};
        let fail = |id: &str| WorkflowEvent::NodeFinished {
            node: NodeId::parse(id).unwrap(),
            ok: false,
            output: String::new(),
        };
        let term = WorkflowEvent::Terminal {
            outcome: WorkflowOutcome::Completed,
            output: "VERDICT: APPROVE".into(),
        };
        // codex leg collapses at the DRAFT only (refine then "succeeds" on garbage) → still 1 failed leg.
        let ev = vec![fail("reviewer_codex_draft"), term.clone()];
        assert_eq!(reduce(&ev).2, 1);
        // both draft + refine fail → deduped to 1.
        let ev2 = vec![fail("reviewer_codex_draft"), fail("reviewer_codex"), term.clone()];
        assert_eq!(reduce(&ev2).2, 1);
        // standard: two distinct reviewer ids → 2; a synth failure never counts.
        let ev3 = vec![fail("reviewer_codex"), fail("reviewer_claude"), fail("synth"), term];
        assert_eq!(reduce(&ev3).2, 2);
    }
```

- [ ] **Step 2: Run — verify it fails**

Run: `cargo test -p a2a-bridge reduce_dedups 2>&1 | head`
Expected: FAIL — `ev2` returns 2 (current code counts every failed node).

- [ ] **Step 3: Implement the dedup**

Replace `reduce` in `review.rs` with:

```rust
/// PURE. Reduce drained workflow events → (completed, terminal_output, failed_reviewer_legs). A failed
/// non-`synth` node is a degraded reviewer leg; draft + refine of the same leg dedup (strip `_draft`).
pub fn reduce(events: &[bridge_workflow::executor::WorkflowEvent]) -> (bool, String, usize) {
    use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome};
    let (mut completed, mut output) = (false, String::new());
    let mut failed_legs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in events {
        match e {
            WorkflowEvent::NodeFinished { node, ok, .. } if !ok && node.as_str() != "synth" => {
                let id = node.as_str();
                let leg = id.strip_suffix("_draft").unwrap_or(id);
                failed_legs.insert(leg.to_string());
            }
            WorkflowEvent::Terminal { outcome, output: o } => {
                completed = matches!(outcome, WorkflowOutcome::Completed);
                output = o.clone();
            }
            _ => {}
        }
    }
    (completed, output, failed_legs.len())
}
```

- [ ] **Step 4: Run the test + the existing reduce test**

Run: `cargo test -p a2a-bridge reduce_`
Expected: PASS (`reduce_dedups_draft_and_refine_into_one_leg` + the existing `reduce_counts_failed_reviewers_and_terminal`).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add bin/a2a-bridge/src/review.rs
git commit -m "fix(review): reduce() counts distinct reviewer legs (dedup _draft+refine) — thorough degradation accuracy"
```

---

### Task 4: Relocate depth parse/format helpers into `review.rs` + add thorough/auto arms

Move `parse_depth_flag`/`depth_from_checkpoint`/`depth_to_forced_str` out of `main.rs` into `review.rs` as `Depth::{parse_flag,from_forced_str,to_forced_str}`, adding the `thorough` + `auto` arms, so config (`default_depth`) and the checkpoint share one impl. The CLI arg type stays `Depth` here (the `Option<Depth>` seam is Task 5).

**Files:**
- Modify: `bin/a2a-bridge/src/review.rs` (add 3 assoc fns + tests)
- Modify: `bin/a2a-bridge/src/main.rs:635-667` (delete locals), `:772,702` (call sites), `:1672,1889,1891,1893` (checkpoint), `:4204` test
- Modify: `bin/a2a-bridge/src/config.rs` (`default_depth` field via `Depth::parse_flag`)

- [ ] **Step 1: Write failing review.rs tests**

Add to `review.rs` tests:

```rust
    #[test]
    fn depth_parse_flag_and_forced_str_round_trip() {
        assert_eq!(Depth::parse_flag("auto"), Ok(Depth::Auto));
        assert_eq!(Depth::parse_flag("light"), Ok(Depth::Forced(Tier::Light)));
        assert_eq!(Depth::parse_flag("standard"), Ok(Depth::Forced(Tier::Standard)));
        assert_eq!(Depth::parse_flag("thorough"), Ok(Depth::Forced(Tier::Thorough)));
        assert!(Depth::parse_flag("bogus").is_err());

        assert_eq!(Depth::from_forced_str(Some("thorough")), Depth::Forced(Tier::Thorough));
        assert_eq!(Depth::from_forced_str(None), Depth::Auto);
        assert_eq!(Depth::from_forced_str(Some("bogus")), Depth::Auto);

        assert_eq!(Depth::Forced(Tier::Thorough).to_forced_str(), Some("thorough".to_string()));
        assert_eq!(Depth::Auto.to_forced_str(), None);
    }
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test -p a2a-bridge depth_parse_flag 2>&1 | head`
Expected: compile error — assoc fns undefined.

- [ ] **Step 3: Implement the assoc fns in `review.rs`**

Add to `impl Depth` (next to `resolve`):

```rust
    /// Parse a `--depth` / `default_depth` value. "auto"→Auto, else Forced(tier); unknown→Err.
    pub fn parse_flag(s: &str) -> Result<Depth, String> {
        match s {
            "auto" => Ok(Depth::Auto),
            "light" => Ok(Depth::Forced(Tier::Light)),
            "standard" => Ok(Depth::Forced(Tier::Standard)),
            "thorough" => Ok(Depth::Forced(Tier::Thorough)),
            other => Err(format!(
                "--depth: unknown value {other:?} (expected auto|light|standard|thorough)"
            )),
        }
    }

    /// Reconstruct from a checkpoint `forced_depth` string; `None`/unknown → Auto.
    pub fn from_forced_str(s: Option<&str>) -> Depth {
        match s {
            Some("light") => Depth::Forced(Tier::Light),
            Some("standard") => Depth::Forced(Tier::Standard),
            Some("thorough") => Depth::Forced(Tier::Thorough),
            _ => Depth::Auto,
        }
    }

    /// Serialize to a checkpoint `forced_depth` string; Auto → None.
    pub fn to_forced_str(self) -> Option<String> {
        match self {
            Depth::Forced(Tier::Light) => Some("light".into()),
            Depth::Forced(Tier::Standard) => Some("standard".into()),
            Depth::Forced(Tier::Thorough) => Some("thorough".into()),
            Depth::Auto => None,
        }
    }
```

- [ ] **Step 4: Delete the `main.rs` locals and update call sites**

Delete `parse_depth_flag` (635-647), `depth_from_checkpoint` (649-657), `depth_to_forced_str` (659-667) from `main.rs`. Update the three callers:
- Line ~702 and ~772: `depth = Some(parse_depth_flag(Some(val.as_str()))?)` → `depth = Some(review::Depth::parse_flag(val.as_str())?)`; line ~772 `depth = parse_depth_flag(...)?` → `depth = review::Depth::parse_flag(val.as_str())?`. (Note: `parse_flag` returns `Result<Depth, String>`; `?` needs `String: Into<BoxError>` — it is, via `From<String>`.)
- Line ~1672: `forced_depth: depth_to_forced_str(depth)` → `forced_depth: depth.to_forced_str()`.
- Lines ~1889-1893: `depth_to_forced_str(depth_override)` → `depth_override.to_forced_str()`; `depth_from_checkpoint(...)` → `review::Depth::from_forced_str(...)`.

- [ ] **Step 5: Update the main.rs checkpoint test**

Replace `depth_from_checkpoint_maps_all_cases` (line ~4204) with:

```rust
    #[test]
    fn depth_from_forced_str_maps_all_cases() {
        assert_eq!(review::Depth::from_forced_str(Some("light")), review::Depth::Forced(review::Tier::Light));
        assert_eq!(review::Depth::from_forced_str(Some("standard")), review::Depth::Forced(review::Tier::Standard));
        assert_eq!(review::Depth::from_forced_str(Some("thorough")), review::Depth::Forced(review::Tier::Thorough));
        assert_eq!(review::Depth::from_forced_str(None), review::Depth::Auto);
        assert_eq!(review::Depth::from_forced_str(Some("bogus")), review::Depth::Auto);
    }
```

- [ ] **Step 6: Add `default_depth` to config**

In `config.rs`, add a default fn:

```rust
fn default_depth_str() -> String {
    "auto".to_string()
}
```

In `ReviewToml` add `#[serde(default = "default_depth_str")] pub default_depth: String,`. In `ReviewConfig` add `pub default_depth: review::Depth,`. (Add `use crate::review;` at the top of `config.rs` if not present — check existing imports; `review` is a sibling module, reference as `crate::review`.) In `to_config`, parse it after the band validation:

```rust
        let default_depth = crate::review::Depth::parse_flag(&self.default_depth)
            .map_err(|e| ConfigError::Registry(format!("[review] default_depth: {e}")))?;
```

and add `default_depth,` to the `Ok(ReviewConfig { ... })` literal.

- [ ] **Step 7: Run the gate**

Run: `cargo test -p a2a-bridge depth_ review_ && cargo clippy -p a2a-bridge --all-targets -- -D warnings`
Expected: PASS; no warnings (confirms no dangling references to the deleted locals).

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add bin/a2a-bridge/src/review.rs bin/a2a-bridge/src/main.rs bin/a2a-bridge/src/config.rs
git commit -m "refactor(review): relocate depth parse/format to review.rs; add thorough/auto arms + [review].default_depth"
```

---

### Task 5: `Option<Depth>` override seam (owner/caller/constructor) + resume replay-correctness

Thread `Option<Depth>` so absent `--depth` (None) is distinct from `--depth auto` (Some(Auto)); resolve the fresh-start default from `[review].default_depth`; fix resume so `--depth auto` can clear a forced checkpoint.

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (ImplementArgs, parse_implement_args, implement_cmd, implement_resume_cmd, usage strings)

- [ ] **Step 1: Write the failing resume-precedence unit test**

Add a pure helper + test in `main.rs` tests. First the helper near `run_review_step` (so it is unit-testable without the full resume path):

```rust
/// PURE. Resume depth precedence: no `--depth` (None) → the checkpoint's stored depth; an explicit
/// `--depth` overrides AND becomes the new persisted depth (`Some(Auto)` clears a forced checkpoint).
fn resolve_resume_depth(
    flag: Option<review::Depth>,
    checkpoint: Option<&str>,
) -> review::Depth {
    match flag {
        None => review::Depth::from_forced_str(checkpoint),
        Some(d) => d,
    }
}
```

Test:

```rust
    #[test]
    fn resume_depth_precedence() {
        use review::{Depth, Tier};
        // no flag → checkpoint value
        assert_eq!(resolve_resume_depth(None, Some("thorough")), Depth::Forced(Tier::Thorough));
        assert_eq!(resolve_resume_depth(None, None), Depth::Auto);
        // explicit flag overrides; --depth auto CLEARS a forced checkpoint
        assert_eq!(resolve_resume_depth(Some(Depth::Auto), Some("thorough")), Depth::Auto);
        assert_eq!(resolve_resume_depth(Some(Depth::Forced(Tier::Light)), Some("thorough")), Depth::Forced(Tier::Light));
    }
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test -p a2a-bridge resume_depth_precedence 2>&1 | head`
Expected: compile error — `resolve_resume_depth` undefined.

- [ ] **Step 3: Change `ImplementArgs.depth` to `Option<Depth>` + the parser**

`ImplementArgs.depth: review::Depth` → `depth: Option<review::Depth>` (update the doc to mention "None = use [review].default_depth"). In `parse_implement_args`:
- resume path: keep `let mut depth = None;` and `depth = Some(review::Depth::parse_flag(val.as_str())?)`, then return `depth,` (drop the `.unwrap_or(review::Depth::Auto)` at line ~718 → just `depth`).
- fresh path: change `let mut depth = review::Depth::Auto;` (line 735) → `let mut depth: Option<review::Depth> = None;` and `depth = review::Depth::parse_flag(val.as_str())?` (line 772) → `depth = Some(review::Depth::parse_flag(val.as_str())?)`. The returned struct already has `depth,`.

- [ ] **Step 4: Resolve the fresh-start default in `implement_cmd` after config load**

`implement_cmd` line 1388 `let depth = a.depth;` now yields `Option<Depth>`. For the resume branch, pass it straight through (Task changes the resume signature below). For the fresh branch, resolve against config AFTER `review_cfg` is built. Find where `review_cfg` is constructed in the fresh path and add, just before the checkpoint/loop uses `depth`:

```rust
    // Owner default: an absent --depth falls through to [review].default_depth (else Auto).
    let default_depth = match &review_cfg {
        Some(Ok(rc)) => rc.default_depth,
        _ => review::Depth::Auto,
    };
    let depth = depth.unwrap_or(default_depth);
```

(Here `depth` is shadowed from `Option<Depth>` to the resolved `Depth`, used at the `forced_depth: depth.to_forced_str()` checkpoint site and threaded into the loop. `review_cfg` is the `Option<Result<ReviewConfig, _>>` already in scope for `ProdEffects`.)

- [ ] **Step 5: Change the resume signature + use the pure precedence**

`implement_resume_cmd(.., depth_override: review::Depth)` → `depth_override: Option<review::Depth>`. The call from `implement_cmd` (line ~1402) passes `depth` (the `Option<Depth>` from `a.depth`, unmodified — resume must NOT consult config default). Replace the depth block at lines 1887-1894 with:

```rust
    // Resume precedence (replay-correct): None → checkpoint; Some(d) → override AND persist (Some(Auto)
    // clears a forced checkpoint). forced_depth=None re-sizes each attempt against current thresholds.
    let depth = resolve_resume_depth(depth_override, ck.forced_depth.as_deref());
    if depth_override.is_some() {
        prod_ckpt.ck.forced_depth = depth.to_forced_str();
        let _ = implement_resume::save_checkpoint(&clone, &prod_ckpt.ck);
    }
```

- [ ] **Step 6: Update the usage strings**

In `IMPLEMENT_USAGE` (line 622-633): the synopsis `[--depth light|standard]` → `[--depth auto|light|standard|thorough]`, and the `--depth` line → `  --depth         review depth: auto|light|standard|thorough (default: [review].default_depth, else auto)`.

- [ ] **Step 7: Run the gate**

Run: `cargo test -p a2a-bridge && cargo clippy -p a2a-bridge --all-targets -- -D warnings`
Expected: PASS; no warnings.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(review): Option<Depth> seam — default_depth owner default + replay-correct resume (--depth auto clears forced)"
```

---

### Task 6: `review-implement-refine.md` prompt + prompt-contract test

**Files:**
- Create: `prompts/review-implement-refine.md`
- Modify: `bin/a2a-bridge/src/main.rs:4220-4254` (prompt-contract test)

- [ ] **Step 1: Create the refine prompt**

Create `prompts/review-implement-refine.md`:

```markdown
You are ONE of two INDEPENDENT reviewers doing a SECOND, deeper pass over a committed code change. Your own
first-pass draft is provided below as `{{input}}`'s reviewer context; treat it as a STARTING MAP, not a ceiling.

READ-ONLY + BOUNDED CONTRACT — follow exactly:
- You MAY use READ-ONLY tools: read files, list dirs, grep/search, and `git diff` / `git log` / `git show`. Also permitted: `git blame`, `git log -L <range>:<file>` (line history), and `git log -S/-G` (pickaxe) to trace why/when code changed.
- **prism (if code-graph nav tools are available — named `mcp__<server>__*` for claude/codex, bare `nav_*` for kiro):** a code-graph (CPG) navigator over THIS repo — prefer it over grep for STRUCTURAL questions. `nav_repo_map` to orient; `nav_callers`/`nav_callees`/`nav_ego_graph` seeded by `{kind:"symbol", name:"X"}` for "who calls X / what breaks if I change X"; `nav_module_deps` for module edges. Read-only.
- If the task input names a `prism review-slice` reference-file path, read it FIRST as a map of where to look, then verify against the code.
- Read ONLY within this repository (your current working directory). Do NOT read outside it.
- Do a thorough, human-style **line-by-line** reading and analysis of the artifact, regardless of its size.
- You may NOT modify anything: no edit/write/create/delete, no builds, formatters, installs, test runs, or any network/shell command beyond the read-only git/search above. When your review is complete, STOP.

REFINE — improve your draft against the code, do not merely restate it:
1. RE-VERIFY each draft finding against the actual diff + surrounding code. Promote a finding whose severity you under-called, demote or DROP a false positive, and correct any location/fix that was wrong.
2. SURFACE what the first pass missed: acceptance gaps the task implies, correctness/edge-cases/broken invariants, and design/architecture/boundary issues. A second pass exists to find what one pass does not.
3. Keep the three dimensions: ACCEPTANCE (delivers the task), CORRECTNESS (bugs/regressions/tests that don't test), DESIGN (module/layer fit, no needless duplication).

OUTPUT: a prioritized, refined list, each finding tagged **BLOCKER / MAJOR / MINOR** with location + the fix.
End with a one-line overall assessment. Do NOT emit a VERDICT line — the synthesizer decides the verdict.

{{input}}
```

- [ ] **Step 2: Add the prompt to the contract test + assert no VERDICT**

In `main.rs::reviewer_prompts_carry_line_by_line_and_git_archaeology`, add `"review-implement-refine.md",` to the `reviewers` array (after `"review-implement.md"`). After the existing `synth` assertion (line ~4253), add:

```rust
        let refine = std::fs::read_to_string(dir.join("review-implement-refine.md")).unwrap();
        assert!(
            !refine.contains("VERDICT"),
            "review-implement-refine must not emit a VERDICT (synth decides)"
        );
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p a2a-bridge reviewer_prompts_carry`
Expected: PASS (refine prompt has `line-by-line`, `git blame`, `log -L`, no `VERDICT`).

- [ ] **Step 4: Commit**

```bash
git add prompts/review-implement-refine.md bin/a2a-bridge/src/main.rs
git commit -m "feat(review): add review-implement-refine.md (thorough second pass) + extend prompt-contract test"
```

---

### Task 7: Wire the `implement-review-thorough` workflow + `[review]` fields into the examples (mirrored)

The example is the dogfood config. The podman parity test (`main.rs:3724`) requires the docker + podman files to be byte-identical except comments / `runtime =` / `allowed_cmds =` lines — so every line added here MUST be added identically to both files.

**Files:**
- Modify: `examples/a2a-bridge.containerized.toml`
- Modify: `examples/a2a-bridge.containerized.podman.toml`

- [ ] **Step 1: Add the thorough workflow to BOTH example files**

Immediately after the `implement-review-light` workflow block (docker file lines 209-222; find the identical block in the podman file), insert in BOTH files, byte-identical:

```toml

[[workflows]]
# implement-review-thorough: 2 reviewers each draft→refine → synth (the large-diff tier). Reuses the
# standard reviewer + synth prompts; the refine pass re-reads the diff line-by-line. Slice attached.
id = "implement-review-thorough"
[[workflows.nodes]]
id = "reviewer_codex_draft"
agent = "codex"
prompt_file = "../prompts/review-implement.md"
inputs = []
[[workflows.nodes]]
id = "reviewer_claude_draft"
agent = "claude"
prompt_file = "../prompts/review-implement.md"
inputs = []
[[workflows.nodes]]
id = "reviewer_codex"
agent = "codex"
prompt_file = "../prompts/review-implement-refine.md"
inputs = ["reviewer_codex_draft"]
[[workflows.nodes]]
id = "reviewer_claude"
agent = "claude"
prompt_file = "../prompts/review-implement-refine.md"
inputs = ["reviewer_claude_draft"]
[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt_file = "../prompts/review-implement-synth.md"
inputs = ["reviewer_codex", "reviewer_claude"]
```

- [ ] **Step 2: Add the `[review]` thresholds + default_depth + bump the timeout in BOTH files**

In the `[review]` block of BOTH files (docker lines 21-30), change `timeout_secs = 900` to `timeout_secs = 1800` and append after `light_max_files = 2`, byte-identical:

```toml
# Thorough-tier trigger: a diff at or above EITHER limit (LLOC over code/infra files only — docs, tests,
# lockfiles, generated files, comments, blanks excluded) runs <workflow>-thorough (draft→refine ×2).
thorough_min_lines = 150
thorough_min_files = 6
# Default review depth when `--depth` is not given: auto | light | standard | thorough.
default_depth = "auto"
```

Also update the timeout comment in BOTH files (it currently justifies 900s) — change "900s gives room" to "1800s gives room (thorough does ~2× passes)". This is a comment line, so the parity test ignores it, but keep them in sync for clarity.

- [ ] **Step 3: Run the parity + load tests**

Run: `cargo test -p a2a-bridge podman_example_parses_validates_and_mirrors_docker`
Expected: PASS (structural remainders byte-identical).

Run: `cargo test -p a2a-bridge --all-targets 2>&1 | tail -5`
Expected: PASS (the containerized example's workflows still load; no count assertion regresses — the init scaffold stays 5).

- [ ] **Step 4: Manually confirm the example loads the new workflow**

Run: `cargo run -p a2a-bridge -- run-workflow --help >/dev/null 2>&1; grep -c "implement-review-thorough" examples/a2a-bridge.containerized.toml examples/a2a-bridge.containerized.podman.toml`
Expected: each file reports `1`.

- [ ] **Step 5: Commit**

```bash
git add examples/a2a-bridge.containerized.toml examples/a2a-bridge.containerized.podman.toml
git commit -m "feat(review): wire implement-review-thorough + [review] thresholds/default_depth (docker+podman mirrored)"
```

---

### Task 8: Full-workspace gate + live DoD documentation

**Files:** none (verification only).

- [ ] **Step 1: Format + clippy the whole workspace**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`
Expected: no diff from fmt; no clippy warnings.

- [ ] **Step 2: Run the full test suite**

Run: `cargo test --workspace 2>&1 | tail -20`
Expected: all tests pass (the new `review.rs` + `config.rs` + `main.rs` tests included).

- [ ] **Step 3: Record the live DoD gate steps (run separately, needs docker + real codex+claude)**

These are run manually against the containerized example (NOT in CI — they need OrbStack + real agent creds). Document the exact commands in the PR description:

```bash
# thorough auto-select: a clone whose task produces ≥150 LLOC of code/infra → tier=thorough
a2a-bridge implement "<task producing a large code diff>" --repo <repo> \
  --config examples/a2a-bridge.containerized.toml
# expect in stderr: review runs implement-review-thorough (4 reviewer nodes + synth), slice written under
#   <clone>/.git/a2a-bridge/review-slices/, VERDICT in the hand-off, checkpoint forced_depth absent (auto).

# forced upgrade on a small diff:
a2a-bridge implement "<tiny task>" --repo <repo> --depth thorough \
  --config examples/a2a-bridge.containerized.toml
# expect: implement-review-thorough runs; checkpoint forced_depth = "thorough".

# forced downgrade + auto-clear on resume:
a2a-bridge implement --resume <id> --depth auto --config examples/a2a-bridge.containerized.toml
# expect: forced_depth cleared to absent; tier re-sized from the diff.
```

- [ ] **Step 4: Commit (if any fmt-only changes) and finish**

```bash
git status --porcelain   # if fmt touched anything:
cargo fmt --all && git add -A && git commit -m "style: cargo fmt --all"
```

Then use **superpowers:finishing-a-development-branch**.

---

## Self-Review

**Spec coverage:** §1 tier model → Task 1; §2 `parse_diff_for_depth`/`counts_toward_depth`/`is_logical_line` → Task 2; §3 topology → Task 7; §4 refine prompt → Task 6; §5 orchestration (sizing Option, slice `!=Light`, `variant_or_fallback`) → Tasks 1+2; §6 override seam + shared parser → Tasks 4+5; §7 config + validation → Tasks 1+4; §8 resume → Task 5; §9 `reduce()` dedup → Task 3. Testing/podman/prompt-embed → Tasks 6+7+8. All spec sections map to a task.

**Placeholder scan:** every code step shows complete code; commands have expected output. No TBD/TODO.

**Type consistency:** `Depth::resolve(Option<(usize,usize)>, usize×4)` defined in Task 1 and called identically in Tasks 1/2; `select_tier(usize×6)` consistent; `parse_diff_for_depth(&str)->(usize,usize)` and `counts_toward_depth(&str)->bool` / `is_logical_line(&str,&str)->bool` consistent across Tasks 2; `Depth::{parse_flag(&str)->Result<Depth,String>, from_forced_str(Option<&str>)->Depth, to_forced_str(self)->Option<String>}` defined Task 4, used Tasks 4/5; `variant_or_fallback`/`resolve_resume_depth` signatures stable. Node ids `reviewer_codex_draft`/`reviewer_codex` align Task 3 (reduce dedup) with Task 7 (workflow). Greenness: each task compiles (no enum non-exhaustiveness or dangling refs left between commits).
