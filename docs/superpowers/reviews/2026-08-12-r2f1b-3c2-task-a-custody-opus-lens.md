I'll start by reading the exact source under review. This is a read-only design pass — no edits, builds, tests, network, or nested helpers.The brief forbids nested helpers, so I'll read the source directly rather than dispatching Explore agents.This is important — the binary already implements a capture-quarantine protocol. Let me read it.I have the full picture. Let me verify two mechanism claims before writing the design.I have everything I need — including a decisive piece of prior art. Writing the design.Design complete. Full document: `/Users/wesleyjinks/.claude/plans/act-as-an-independent-crispy-lynx.md`. I read the exact source; I ran no builds or tests.

Three findings reframe the problem before any design:

**F1 — `revalidate` is two-thirds dead, and the live third doesn't prove the route.** `verify_directory_file` (`fs_custody.rs:1085`) compares only `dev`/`ino`/`btime` — all immutable properties of an open inode — so the two fd self-checks in `revalidate` (`:622`) are tautologies. Only the `openat(parent_fd, root_name)` check does work, and it proves the *parent inode* has a child named `root_name`, not that the configured path names it: the parent is never re-anchored after `open` canonicalized it. Renaming the **parent** leaves every `revalidate` green while the custody operates on a detached subtree. Strictly broader than the closure review's WRONG, and unreachable by any recheck placement.

**F2 — the correct protocol already ships here.** `local_file.rs` implements capture-by-no-replace-into-quarantine (`:45`, `:1285`, `:1325`) with a working crash resolver, and its red test at `:2330` injects a real `RENAME_EXCHANGE`/`RENAME_SWAP` at the last pre-syscall boundary and proves the protocol refuses. The primitive the brief asks about in Q2 exists in this workspace — as the *attacker's*, defeated by capture.

**F3 — `JournalRootCustodyV1` has zero callers outside its own file.** Production ripple for redesigning the policy layer is zero; the mechanism layer must stay signature-stable.

**(1) Invariants.** One organizing rule: every mutation derives its precondition from (A) the syscall carrying it atomically, (B) descriptor addressing, or (C) the operation lock. An identity recheck is none of these — that's the mechanism reason the class is open. Plus an impossibility result that must be stated, not designed around: no syscall on either platform offers an inode-qualified unlink or rename, so provable exactness against an *arbitrary* peer is unreachable by any design.

**(2) Salvage.** The candidate is salvageable and I found no mechanism proving otherwise. KEEP the whole mechanism layer and all 60 pre-existing tests — `rename_child_no_replace` especially, whose compile-time-vs-errno discrimination is what makes capture possible. REVISE `open`/`revalidate`/lock-acquire, salvaging the Round-1 identity plumbing and the lock's `openat→verify→flock` sequence verbatim (the one candidate mutator that's genuinely fd-bound and correct). REPLACE four name-based mutators and `RegularFileIdentityV1`, which conflates "which object" with "how much content" — so a retirement is invalidated by a *legitimate* prior append.

**(3) Q2 answered: no.** `RENAME_EXCHANGE`/`RENAME_SWAP` cannot be the common contract — it's detect-after-commit (our record is already visible before we learn we displaced the wrong object), macOS `RENAME_SWAP` requires both names to exist, support is narrower than `RENAME_NOREPLACE`, and it still selects by name so it doesn't defeat the impossibility result.

**(4) Split.** A1 route/identity → A2 lock → A3 capture+recovery → A4 wire-and-delete, revised in place so no dead duplicate exists. Per-task caps 120/100/130/150.

**(5) Tests.** Hooks fire at the last statement before each syscall, not before a recheck. The load-bearing pair: a replace crashed after capture must **restore** the predecessor while a retire crashed after capture must **finish** — both passing simultaneously is impossible with one quarantine namespace. A naive hoist of `local_file`'s single namespace makes a crashed replacement recover as a completed deletion.

**(6)** Six owner decisions with recommendations; seven residual risks.

DESIGN LENS: NOT READY

Unresolved blockers — three, each closed and with a named resolution:

1. **Birthtime policy (O2) shapes the artifact, not just a setting.** The candidate hard-requires birthtime (`:1091`, `:1169`), so on any filesystem exposing none the journal is unusable. Whether `ObjectIdentityV1.btime` is `Option` with a `Degraded` branch or mandatory is an A1 type decision I cannot make without knowing (a) whether the production journal filesystem exposes birthtime and (b) whether the requirement mandates inode-recycle resistance. Resolution: one command on the target host — `cargo test -p bridge-core journal_root_custody` — plus the requirement citation. I flagged the tmpfs failure as a mechanism-level hypothesis, not an observation; I did not run it.
2. **Whether `Retained` is terminal (O6) changes A4's scope.** If a protective outcome must escalate the journal to a write-blocked mode pending operator resolution, A4 needs an escalation path and its 150-line cap is wrong. This is journal policy the custody layer cannot decide.
3. **Production budget: 500 lines vs the declared 450.** The overage buys the capability probes, recovery, and the reversible-name codec — none of which the candidate has. Needs ratification before dispatch, or A3's recovery splits into a fifth task. Per convergence discipline, sizing is settled before dispatch, not after rejection.

O1 (threat model), O3, O4, and O5 are surfaced with recommendations rather than held as blockers — the brief explicitly permits surfacing those as owner decisions.