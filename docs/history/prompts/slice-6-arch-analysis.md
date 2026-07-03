You are doing an ARCHITECTURE ANALYSIS pass (not a line-review) on the Slice-6 "Event-journal dual-store" design, grounded against the ACTUAL a2a-bridge code (session-cwd = the bridge repo). READ-ONLY: read files, grep, `git log`; do NOT edit/build/test. Be rigorous, structural, and decisive — this is high-effort (xhigh) architecture work and the second lens of a dual-lens pass (a controller analysis already exists; you are the independent second opinion). Do NOT rubber-stamp; do NOT re-litigate the CONVERGED decomposition (OPEN-1 is RESOLVED) — pressure-test the MIGRATION and resolve the open decisions.

The controller's analysis doc is below:

{{input}}

CONTEXT YOU MUST READ (ground every claim in real code with file:line):
- The design-of-record: `docs/superpowers/2026-06-17-orchestration-architecture.md` (OPEN-1, RESOLVED — the binding `OrchEvent`/`OrchResult`/`OrchCommand` schema). Do NOT propose a different schema; the envelope is settled.
- The slicing roadmap row for Slice 6: `docs/superpowers/2026-06-17-orchestration-slicing.md`.
- The Slice-0 minimal types already shipped: `crates/bridge-core/src/orch.rs` (`OrchEvent`/`OrchResult`, flatten+tag, Ser+De). Slice 6 WIDENS these.
- The 4 current event carriers — read each: `Update` (`crates/bridge-core/src/ports.rs:19`), `WorkflowEvent` (`crates/bridge-workflow/src/executor.rs:73`), `translator::Event` (`crates/bridge-workflow/src/translator.rs:25`), `WorkflowProgressFrame` (`crates/bridge-a2a-inbound/src/reattach.rs:36`).
- The seq machinery + dual-store + W3b crash-resume: `crates/bridge-a2a-inbound/src/task_store.rs` (per-task `next_seq`, `record_node_started`/`put_node_checkpoint_sequenced`/`set_terminal_sequenced`, `progress_snapshot`), and the resume path (`resume_working_tasks`, `run_from(seed)`, `finalize_detached`).
- The reattach/SSE projection: `crates/bridge-a2a-inbound/src/reattach.rs` (`DetachedProgressSink`, `TaskProgressHub`, `subscribe_to_task`, Last-Event-ID cursor).

ANALYZE — give the controller a rigorous independent second opinion:

1. **The migration strategy (S6.1–S6.5).** Is projection-first + typed-columns-stay-authoritative the right keystone? Is the S6.1→S6.5 ordering correct, or is there a higher-rework-risk unit that must be pinned FIRST (the project's "risky unit before any consumer pins it" rule)? Would you fold or split any step? Is each step a real additive boundary that leaves the tree green?

2. **Resolve the 5 open decisions** — take a position on each with the code-grounded reason:
   - **D-A (the DoD's "byte-identical"):** exact-old-JSON-wire (project `OrchEvent`→frozen `WorkflowProgressFrame`) vs same-ordered-semantic-events (change the wire to `OrchEvent`, version-bump, update `task watch`). This is THE pivotal call — it sets S6.4's shape + scope. Read `reattach.rs` + the `task watch` client and decide.
   - **D-B (dual-store atomicity):** journal row in the SAME `next_seq` call/transaction as the typed write vs a separate projector table. Check the SQLite transaction boundary + the "alloc-seq-then-durable-write, check-exists-first" invariant in `task_store.rs`.
   - **D-C (source/fan-out + the 4th-path fragmentation):** the translator/local-A2A path does NOT consume `WorkflowEvent`. Does the local A2A SSE path journal at all (it's live-projection today, no persistence), or does only the DETACHED path get the durable journal? Where do the per-producer adapters live?
   - **D-D (W3b invariant):** confirm the serialized journal NEVER becomes a resume input (resume stays on typed checkpoints) — and identify any place the proposed change risks coupling them.
   - **D-E (slicing granularity):** is S6.1–S6.5 the right decomposition?
3. **Hazards the controller missed.** What cross-cutting failure mode does this migration risk that the analysis doc does NOT name? (e.g. seq exhaustion/ordering under concurrent nodes, journal write failure vs typed write success = divergence, snapshot read amplification, broadcast(256) lag interacting with a larger per-event payload, migration of existing on-disk tasks, version skew between a journaling serve and an old `task watch`.) Be concrete with file:line.
4. **What to CUT or DEFER** to keep Slice 6 minimal but non-redesign-forcing. Is anything over-built? Is `OrchCommand` in-scope for Slice 6 or does it belong to S9 (permission blocking)? Is the full `OrchEventKind` set needed now or only the kinds the detached path actually emits?

OUTPUT: a structured architecture critique — (a) migration-strategy verdict + the corrected slicing/order if you'd change it, (b) D-A through D-E rulings each with a one-line code-grounded reason, (c) hazards-the-controller-missed list (file:line), (d) cut/defer list. End with one line: `ARCH VERDICT: sound | sound-with-changes | needs-rework`. Be a co-architect: propose concrete improvements, not just gaps. Do NOT edit any files; this is analysis only.
