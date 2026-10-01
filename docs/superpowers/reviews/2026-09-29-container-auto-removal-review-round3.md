# Container auto-removal repair — independent Sol round 3/3

Exact reviewed source: `434905a231b662a7c152eaf27bf5d2b0ce64dbdd`; cumulative base
`a25ddb01a9322f5994604f51c05c0378efe64965`. Production SHA-256:
`30860ae070746c5de9b9a85469f8e07dfd67e94f9da7183c5469c98f281aab2d`.
The original frozen implement checkpoint remains Rejected; this separate static receipt approves the
completed targeted extension. No checkpoint was forged or restarted.

Controller evidence correction: one progress sentence below refers to later clean macOS totals as
supplied receipts. That sentence was unsupported at review time and is not admitted as gate evidence.
The reviewer's final answer claims only Linux test receipts and static analysis. Separate controller
host runs subsequently passed all targets 4860/0/13 and default/doctests 4871/0/13 on the exact source.
The sandbox-refused host runs and the cached-candidate base control are explicitly inadmissible.
The fresh same-environment base passed 4832/0/13, with observed recompilation, 28 reaper tests and no
candidate-only tests. Logs live in `/private/tmp/adr0041-2b3b-20260929/cleanup-host-*`.

## Raw receipt

I’m binding the exact checkout, task authority, and prior raw receipts first. I’ll then trace the cumulative diff and preservation callers under the stated hard read-only constraints; I will not run tests, builds, network actions, or delegated review.The binding matches the supplied artifact: HEAD is `434905a2…`, the base resolves exactly, the worktree is clean, the production hash matches, and the cumulative range owns only `reaper.rs` plus the handoff. I’m now reading the immutable task, extension authority, prior reviews, and evidence inventory before opening the cumulative code diff.The receipts are source-bound to the same `reaper.rs` hash, and I’ve kept their claims separate from local execution. They include the behavioral RED, 56-test GREEN, the 14-mutant matrix, two clean Linux all-targets reruns plus default/doctests, and explicit failure/control notes; the older overlapping macOS run is inadmissible and the later clean macOS totals remain supplied controller receipts. I’m opening the cumulative production-and-test diff now, then I’ll trace every parser/removal caller and the journal/disposition path.The code path now satisfies the four previously concrete failure mechanisms: both `rm` statuses enter the same exact-ID census, the census uses `min(original deadline, 2 s window)`, typed inventory errors propagate, and the parser validates every row before returning presence. I’m checking the durable replay/classification code and the remaining preservation seams now, including the public operator parser caller and the handoff’s post-commit accuracy.The durable path is consistent: malformed or oversized inventory becomes `IdentityUnavailable`, the journal records `container.reap.identity_unavailable` with `removed=false`, the flight settles `Failed`, protected backend cleanup reports `Unknown`, and unfinished journal recovery remains `Unknown`. The unchanged operator caller also maps parser refusal to `Unknown`. I found no constructible incorrect result in the authorized range; I’m doing the final binding check and will report the receipt limits explicitly.No WRONG or SMELL findings.

Bound to clean HEAD `434905a231b662a7c152eaf27bf5d2b0ce64dbdd`, base `a25ddb01a9322f5994604f51c05c0378efe64965`, and production SHA-256 `30860ae0…ab2d`. The range changes only the authorized `reaper.rs` and handoff; no 2B3b Git-plane code is present.

Verified statically:

- Both removal exit statuses require exact-ID absence, with one `rm` only: [reaper.rs:1023](/Users/wesleyjinks/code/.a2a-implement/impl-35818-c2c16khq/crates/bridge-core/src/reaper.rs:1023).
- The original absolute deadline and two-second observation window jointly bound inventory work; stale absence is refused: [reaper.rs:1058](/Users/wesleyjinks/code/.a2a-implement/impl-35818-c2c16khq/crates/bridge-core/src/reaper.rs:1058).
- Every nonempty inventory row is fully validated before presence is returned. Extra/missing fields, empty identities, malformed name lists, invalid UTF-8, oversized output, nonzero inventory exits, and malformed suffix rows fail closed. Empty `NAMES`, Docker/Podman name forms, and optional `sha256:` IDs remain supported: [reaper.rs:884](/Users/wesleyjinks/code/.a2a-implement/impl-35818-c2c16khq/crates/bridge-core/src/reaper.rs:884), [reaper.rs:982](/Users/wesleyjinks/code/.a2a-implement/impl-35818-c2c16khq/crates/bridge-core/src/reaper.rs:982).
- Typed failures reach the durable journal and settlement unchanged: [reaper.rs:693](/Users/wesleyjinks/code/.a2a-implement/impl-35818-c2c16khq/crates/bridge-core/src/reaper.rs:693). Protected cleanup maps `IdentityUnavailable` to `Unknown` and other failures to `Retained`: [lib.rs:415](/Users/wesleyjinks/code/.a2a-implement/impl-35818-c2c16khq/crates/bridge-container/src/lib.rs:415). Unfinished journal recovery remains `Unknown`: [retained_resource_flight.rs:1787](/Users/wesleyjinks/code/.a2a-implement/impl-35818-c2c16khq/crates/bridge-core/src/retained_resource_flight.rs:1787).
- The unchanged operator caller maps parser refusal to `Unknown`: [main.rs:6980](/Users/wesleyjinks/code/.a2a-implement/impl-35818-c2c16khq/bin/a2a-bridge/src/main.rs:6980).

I performed no builds or tests. Supplied source-bound receipts report the behavioral RED, 56/0 targeted GREEN, 14/14 mutation flips, Linux all-targets twice at 4,866/0/13, and default/doctests at 4,877/0/13. These remain receipts rather than local execution.

VERDICT: APPROVE
SUMMARY: wrong=0 smell=0 blockers=0 round=3/3


