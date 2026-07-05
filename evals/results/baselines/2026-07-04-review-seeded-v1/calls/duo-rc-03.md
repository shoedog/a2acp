**Merged Review: rc-03 — `Registry::register`**

Both lenses ran successfully (correctness/codex + architecture/claude). They agree on the key fact and do not conflict: there is **no WRONG/panic defect** — the `.unwrap()` at `src/lib.rs:22` is provably safe today (the `insert` on the preceding line ran under `&mut self`, and nothing between insert and get reenters user code or another thread). Codex is right that no correctness defect exists; Claude is right that the API is structurally mis-designed. Everything below is SMELL/design, not WRONG. There are no BLOCKERs.

---

**MAJOR — Returned `&str` borrows the whole registry, locking it for the handle's lifetime (`src/lib.rs:17`, `:22`).**
The return type `-> &str` is tied to `&self`, so the handle holds an immutable borrow of `self.agents`. Concrete exposing case: `let a = reg.register("a", cmd_a); let b = reg.register("b", cmd_b);` fails to compile — `a`'s borrow blocks the second call's `&mut self`. Only "register one, use it immediately" survives; "register several and keep the handles" does not. **Fix:** return an owned `String`, or `()` and let callers `get`, so the result isn't bound to a registry-wide borrow.

**MAJOR — The returned value is strictly less useful than the argument the caller already owns (`src/lib.rs:17`).**
The caller passes `cmd: &str` (their lifetime) and gets back the same content with a *worse* lifetime plus the lock from the finding above. The only thing added over the input is "this is the stored copy." If `register` returns anything, it should return something the caller doesn't already have — namely the displaced previous command (see next). **Fix:** reconsider whether `register` returns at all; if it does, return the prior value.

**MINOR — `insert`'s return value (the old command) is discarded, closing off any overwrite/uniqueness invariant (`src/lib.rs:18`).**
`HashMap::insert` yields `Option<String>`; dropping it means a second `register("a", …)` silently clobbers the first with no signal. Adding a "reject duplicate" or "return previous" policy later would require a signature change. **Fix:** if overwrite is intended, document it; otherwise surface the prior value or gate via `entry`.

**MINOR — Redundant insert-then-get double-hashes the key and creates the self-inflicted `unwrap` (`src/lib.rs:18`, `:22`).**
`id` is hashed and the bucket walked twice. **Fix:** use the `entry` API to get a reference to the just-stored value in one probe, eliminating both the second lookup and the `unwrap`.

**MINOR — `SAFETY:` convention misapplied to safe code (`src/lib.rs:19–21`).**
`SAFETY:` is the Rust convention for justifying `unsafe`; on a safe `.unwrap()` it mis-trains readers and can trip `unnecessary_safety_comment`. The comment also memorializes an invariant that exists only because of the insert-then-get choice. **Fix:** remove the `unwrap` via `entry`, or relabel `// INVARIANT:` if the comment must stay.

**MINOR — No tests for the new public API (`src/lib.rs:17`).**
The crate ships `register` with zero coverage. **Fix:** add unit tests proving `register("id", "cmd")` returns `"cmd"`, `get("id")` sees the same value, and re-registering an id stores the replacement (which also pins down the overwrite behavior from the finding above).

---

**Verdict:** Correct and panic-free as written; ship-blocking nothing, but redesign the return contract before relying on it — return an owned/void result and switch to `entry` to drop the double-lookup, its `unwrap`, and the misapplied `SAFETY:` comment, then add the missing tests. Address the 2 MAJORs to avoid baking in an unusable signature.