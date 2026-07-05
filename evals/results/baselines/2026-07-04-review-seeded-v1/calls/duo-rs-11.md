**Merged Review: rs-11 — `RunContext::resolve`**

Both lenses (correctness/Codex, architecture/Claude) ran successfully and agree on the core defect. Prioritized findings:

---

**1. BLOCKER (WRONG) — `src/lib.rs:19-21` — `resolve` ignores `self.session_cwd` and anchors to process cwd.**
The method computes `base` from `std::env::current_dir()` and never touches `self.session_cwd`, leaving the field dead. This is the exact inversion `RunContext` was built to prevent: one `serve` drives many repos, so the process cwd is unrelated to (and usually wrong for) any given run. Concrete failure: serve launched from `/bridge` (or `/opt/bridge`), a run targets `/repos/app`; `resolve("task.md")` returns `/bridge/task.md` instead of `/repos/app/task.md` — the agent reads/writes the wrong repository (or nothing).
*Fix:* `self.session_cwd.join(rel)`; delete the `current_dir()` call.

**2. MAJOR (WRONG) — `src/lib.rs:19-22` — no containment enforcement; paths can escape `session_cwd`.**
The contract promises every resolved path lands *under* `session_cwd`, but `PathBuf::join` gives no such guarantee: an absolute `rel` (`/etc/passwd`) discards the base and is returned verbatim, and `../other/task.md` climbs out of the repo. Since `rel` comes from a task-spec the agent then reads/writes, this is a traversal / sandbox-escape boundary that is simply absent — and it belongs on this method, the object that defines "the repo the agent may touch."
*Fix:* after joining, reject absolute paths and `..` components, or `canonicalize` and verify the final path is prefixed by `session_cwd`.

**3. MAJOR (WRONG) — `src/lib.rs:20` — `unwrap_or_default()` degrades a hard error into corrupted output.**
`current_dir().unwrap_or_default()` yields an empty `PathBuf` on failure (deleted cwd, permission loss), so `base.join(rel)` returns the bare *relative* `rel` — silently violating the doc comment's "absolute path" guarantee. (Codex did not flag this; Claude is right that the error is swallowed rather than surfaced.) Note: fixing #1 removes this fallible call entirely, since `session_cwd` needs no lookup — that is the real fix, but the pattern of degrading errors to defaults is the smell to avoid.

**4. MINOR (SMELL) — `src/lib.rs:19` — infallible `-> PathBuf` signature forecloses the validation seam.**
Enforcing containment (#2) or canonicalizing symlinks is inherently fallible, so the signature will have to become `-> Result<PathBuf, _>` the moment the invariant is enforced — a breaking change to every future caller. With zero callers today, switching to `Result` now makes the boundary honest and keeps the containment check addable without churn.

**5. MINOR (SMELL) — `src/lib.rs` — no regression tests pin the contract.**
Add tests where process cwd differs from `session_cwd` (the core case), plus negative cases for `../` traversal and absolute inputs.

---

**Verdict:** REJECT — ship only after fixing the BLOCKER (resolve against `session_cwd`, not process cwd), which also dissolves the error-swallowing MAJOR; land the containment invariant in the same change since this boundary object owns it.