**Merged Review: rs-02 — `SandboxConfig::validate` vs `effective_mounts()`**

Both lenses (correctness + architecture) succeeded and independently converged on the same escape. Merged and prioritized below.

---

**BLOCKER [WRONG] — `validate()` checks a different mount set than `effective_mounts()` bind-mounts; sandbox escape via `overrides.extra_mounts`.**
- Location: `src/lib.rs:28` (validate loop over `self.mounts`) vs `src/lib.rs:40-43` (effective_mounts unions `self.mounts` + `self.overrides.extra_mounts`).
- Issue: `validate()` only iterates `self.mounts` and never inspects `overrides`, but `effective_mounts()` — the set actually bind-mounted at runtime — also includes `overrides.extra_mounts`. Concrete input: `SandboxConfig { allowed_root: "/srv/agent", mounts: [], overrides: Some(Overrides { extra_mounts: [Mount { host: "/etc" }] }) }` returns `Ok(())` from `validate()` yet `effective_mounts()` yields `["/etc"]`, which gets mounted. This directly violates the stated contract ("every path `effective_mounts()` will bind-mount MUST pass `under_root`") and is a full escape via the exact feature this diff adds.
- Fix: make `effective_mounts()` the single source of truth and have `validate()` iterate `self.effective_mounts()` (or otherwise validate the identical set). Add a regression test for an escaping override mount (base under root, override outside root → must be rejected). Both reviewers agree; no disagreement to resolve.

**MAJOR [SMELL] — no single "all mounts" seam; every future mount source re-opens this hole.** (Claude, architecture)
- Location: the split between `validate()` (`src/lib.rs:28`) and `effective_mounts()` (`src/lib.rs:40`).
- Issue: mount *materialization* and mount *validation* are two independent walks over different fields. Any future layer (a `defaults` tier, env-derived mounts, a second override slot) again requires the author to remember to update `validate()`; the type system gives no help, so this class of bypass recurs. This is the structural root cause of the BLOCKER, and adopting the BLOCKER's fix (validate consumes `effective_mounts()`) also closes this — worth calling out so the fix is done structurally, not by patching one field.
- Fix: collapse to one iterator (e.g. `fn all_mount_hosts(&self) -> impl Iterator<Item=&Path>`) consumed by both `validate()` and the runtime mounter, so "what we check" is coupled to "what we mount" by construction.

**MAJOR [WRONG, pre-existing — diff amplifies it] — `under_root` is a lexical prefix check; `..` and symlinks escape.** (Claude, architecture)
- Location: `src/lib.rs:22-24`.
- Issue: the check relies on `Path::starts_with`, comparing components without resolving `..` or symlinks. `allowed_root="/srv/agent"` with host `"/srv/agent/../../etc"` passes yet resolves to `/etc`. This predates the diff, but the diff routes a *less-trusted, per-agent* override source through the same weak check, widening exposure. (Flagged only by the architecture lens — the correctness lens scoped to the diff-introduced defect and did not raise it; the finding is valid and should be fixed.)
- Fix: normalize/`canonicalize()` both `allowed_root` and each host before the prefix test; decide symlink policy explicitly. Note the TOCTOU caveat — canonicalize-then-mount is still racy — but at minimum normalize `..`.

**MINOR [SMELL] — `Overrides` participates in a security invariant but carries no validation responsibility.** (Claude, architecture)
- Location: `src/lib.rs` (new `Overrides` struct, `src/lib.rs:9-11` region) / the `Option<Overrides>` field.
- Issue: the new struct adds state that affects the security gate but has no enforcement of its own; the unchecked field hides behind `Option`. Dissolves once the single-seam fix (MAJOR #1) lands.
- Fix: no separate action needed if the seam is adopted; otherwise ensure the override field cannot be added without a validation path.

---

**Verdict:** REJECT — do not merge. Fix the BLOCKER by making `validate()` and `effective_mounts()` share one mount enumeration (which also resolves the seam MAJOR), and normalize paths in `under_root` before the prefix check.