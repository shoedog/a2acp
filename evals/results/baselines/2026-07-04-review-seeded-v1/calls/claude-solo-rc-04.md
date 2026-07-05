I'll ground the review in the actual fixture state, then analyze.The fixture matches the diff exactly; it's a standalone synthetic crate (`rc04_bounded_producer`) with nothing else in scope. Review below.

---

## Architecture review: rc-04 `Collector`

**1. BLOCKER — WRONG (under retry) — `collect()` reads a fixed *count* (`take(self.nodes)`), but the real termination condition is "every live node has reported."** `src/lib.rs:37`
The whole drain logic equates *message count* with *node count*. That equality holds only if the unenforced prose invariant "each node sends exactly one result" is perfect. This codebase already ships a feature that breaks it — E6 opt-in **node-retry** re-executes nodes. If any node sends twice (e.g. sends, then a transient post-send failure triggers a re-run that sends again), total messages = `nodes + k`; `take(nodes)` keeps only the first `nodes` in *arrival order* — so you get a **duplicate for the retried node and zero for whichever node's message arrived last**, while `len == nodes` makes the result look complete. Silent, high-consequence data corruption in an orchestrator. Direction: drain to disconnect and validate one-result-per-node-id (dedup/reject by `NodeResult.node`), surfacing duplicates as an error rather than truncating.

**2. MAJOR — SMELL — no completion seam: `collect()` cannot distinguish "all reported" from "partial."** `src/lib.rs:35-38`
In a real DAG a node whose dependency failed is *skipped* and never sends — so fewer than `nodes` messages exist. `take(nodes)` then terminates early via channel disconnect and returns a short `Vec` with **no signal that it's partial and no way to name the missing node(s)**. The contract "each node sends exactly one result" assumes away normal DAG skip/fail behavior. Return type `Vec<NodeResult>` erases the one-per-node invariant and pushes reconstruction onto every caller. Direction: return a keyed/validated structure (`HashMap<NodeId, NodeResult>` + explicit `missing: Vec<NodeId>`), or a `Result` that fails when the expected reporter set isn't satisfied.

**3. MAJOR — SMELL — unbounded hang: `collect()` blocks forever with no timeout or cancellation seam.** `src/lib.rs:37`
If one node holds its `Sender` and never sends (the exact hang this codebase's watchdog exists to catch), `take(nodes)` blocks on the Nth item indefinitely and wedges the entire workflow. There's no `recv_timeout`, and no hook for the per-turn cancellation tokens used elsewhere in this repo — the collector is uninterruptible. Direction: drain via `recv_timeout`/select against a cancel token, returning the partial set plus the still-outstanding node ids.

**4. MAJOR — SMELL — `NodeResult` has no failure representation.** `src/lib.rs:4-7`
`output: String` only — a failed node cannot report failure through this channel; it must either fabricate a success `output` or stay silent (feeding #2). The collector is the natural join point for per-node outcomes, so failure belongs in this type. Direction: make `output` a success/failure sum (`Result<String, NodeError>` or an enum) so `collect()` returns node *outcomes*, not just node *outputs*.

**5. MINOR — SMELL — `sender()` hands out a raw clonable `Sender`, so "exactly once" is structurally unenforceable.** `src/lib.rs:30-32`
A multi-shot `Sender<NodeResult>` per node lets the type system permit 0, 1, or N sends — the root cause of #1. Direction: wrap each handle as a single-use, node-keyed reporter (consumed on send) so the invariant lives in the type, not a doc comment.

**6. MINOR — SMELL — the diff's safety rationale answers the wrong question, and its one guard is debug-only.** `src/lib.rs:9-12, 24`
"Unbounded is safe because the producer is bounded to `nodes` messages" is true for *memory* (≤64 tiny messages — bounded either way), but it frames bounded-producer as the load-bearing property when the actual load-bearing (and unenforced) property is *exactly-once-per-node*. The comment steers a correctness-only reviewer toward memory and away from the real risk. Relatedly, `debug_assert!(nodes <= MAX_NODES)` — the single guard the design cites — is compiled out in release, and the claimed load-time validation lives outside this unit. Direction: document/enforce the exactly-once invariant, and either validate `nodes` with a real check or stop presenting the debug assert as the enforcement point. (Credit where due: `drop(self.tx)` before draining is correct and necessary to allow disconnect.)

---

**Verdict:** Correctness-clean on the happy path, but the design silently converts two in-repo realities — node-retry (over-send) and skipped-on-failed-dep nodes (under-send) — into wrong or partial results with no completion, failure, or cancellation seam; `take(self.nodes)` is the wrong termination primitive. **Request changes.**