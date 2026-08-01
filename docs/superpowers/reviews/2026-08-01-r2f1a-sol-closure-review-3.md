I’m using the checked-in `a2a-bridge-operator` skill because this repository requires it for bridge workflow review. I’ll first verify the frozen checkout identity and custody; any mismatch will end the review immediately with REJECT. After that I’ll inspect the complete named artifacts, parent diff, and only the source seams needed for W1–W6.The operator skill’s hard-read-only guidance is consistent with this checkpoint: no live turn, no provider selection, and roadmap state remains authoritative. I’m now binding the review to the exact checkout, commit parent, cleanliness, and both supplied digests before reading the design content.Identity and custody match exactly: canonical cwd equals the Git toplevel, HEAD and parent match, the tree is clean (including untracked files), and both SHA-256 values match. I’m proceeding with the cumulative review; no identity rejection is triggered.The focused checkpoint is completely read. Its W1 repair is explicit about publish-before-retire and same-slot swaps, fresh binding per candidate/real turn, exact-bound invalidation, and digest-keyed cache; W2–W5 also have concrete schema, materialization, classifier, and encoder mechanisms. I’m now testing those claims against the frozen source and the controlling owner/parent/roadmap contracts, including removed text in the complete parent diff.The cumulative artifact and all authority documents are now read. I’m finishing the parent diff and tracing the exact registry, SQLite, collision, and encoder seams in current source; the central checks are whether the repaired contracts bind to executable ownership without introducing a new race or false invariant.The source trace has isolated three concrete residual seams: a warm API entry can still serve a removed model, configured-store accounting is not conservative for arbitrary-ID overflow/WAL bytes, and the repaired encoder’s mandatory regression contradicts the fallback it specifies. I’m doing the final severity/risk pass now, including the credential-identity threat case and a last custody recheck.## Identity and custody

PASS, verified again immediately before verdict:

- cwd and repository root: `/private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo`
- clean HEAD: `2bcbd524a39ebe7edb5928655681fbe7acad29e5`
- parent: `6987ad3ff29946d894389c5babf938771a1f52de`
- focused artifact SHA-256: `36a10dcb9e74e768cd857d5d182432741def4893b3290b443f4d4e5790e0cbbe`
- roadmap SHA-256: `87f4ce2640c3f6a56fb722a652076759cf879aa2c72c7a5e9b75b48d54cc2e86`
- worktree status: clean, including untracked files

## W1–W6 closure

| Item | Result | Mechanism-level proof |
|---|---|---|
| W1 — provider-effect identity and binding | **PARTIAL** | The two durable digests, lease-first bind, exact slot-Arc map revalidation, exact-entry token, bound resolution, per-candidate rebinding, exact invalidation, dual-digest cache key, and persisted resume/replay identity close the stated publish-before-retire, same-slot swap, fallback, retry, resume, and batch races. Two executable gaps remain: a warm API backend can retain a removed model despite a successful bind, and the persisted unkeyed digest is an offline verifier for literal MCP credentials. |
| W2 — truthful `WITHOUT ROWID` invariant | **FIXED** | Bundled SQLite is 3.46.0. Its `index_list` implementation emits every table index and reports primary-key origin as `pk`; the repaired invariant correctly requires one unique nonpartial PK entry, exact `index_xinfo` key order, no other origin, and no separately rooted `sqlite_schema` index. |
| W3 — arbitrary-node-ID physical admission | **PARTIAL** | Platform admission now uses hard `max_page_count`, supported rollback journals, `cache_spill=OFF`, full placeholder materialization, transaction-visible `page_count`, rollback, and post-commit verification. That closes the false static formula. Configured stores still charge exact key bytes plus a fixed 256-byte “B-tree overhead” and no WAL reserve, which is not conservative for arbitrary IDs. |
| W4 — collision classification | **FIXED** | The fourteen-variant total classifier assigns all thirteen availability/capacity reasons fail-open and `Collision` alone fail-closed. A wildcard is forbidden, so a new enum variant cannot inherit a disposition silently. Reservation, lineage, lease, replay, and terminal-conflict producers are explicitly routed through collision fixtures. |
| W5 — deferred encoder evidence | **PARTIAL** | The fallback now preserves original class/code, sticky acceptance, ancestry, trigger identity, and a deepest bounded suffix while marking overflow separately; the 1,978-byte ceiling remains within 2,048. Its mandatory comparison test nevertheless contradicts the specified fallback by requiring `evidence_overflow` to be the only changed field even though the fallback must drop `dependency_set` and may change the cause fields. |
| W6 — roadmap custody | **FIXED** | The authoritative cursor consistently says design repaired, one closure pending, and implementation unauthorized. Historical rejection text is labeled as history; active status, next action, program table, and handoff agree. |

## WRONG findings

### WRONG W1-A — BLOCKER: a bound API attempt can still use a removed model

Constructible state:

1. An API entry has `model="M"` and an earlier workflow initializes its warm backend.
2. Hot reload removes `model`. Current registry reconciliation treats this as config-only and retains the same backend slot.
3. A new R2f1a attempt freezes and successfully binds the current `model=None` entry.
4. It configures the API session with `None`.
5. The API backend interprets that `None` as “no session override” and falls back to its spawn-time `cfg.model="M"`.

The provider therefore receives `M` while the frozen candidate is `None`. This defeats the central claim that the exact bound entry is the actual call identity.

Mechanism and location:

- The design classifies model as selection-only and promises that bound `SessionSpec` values control dispatch at [focused boundary:576](</private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo/docs/superpowers/plans/2026-08-01-r2f1a-focused-boundary.md:576>) and [focused boundary:857](</private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo/docs/superpowers/plans/2026-08-01-r2f1a-focused-boundary.md:857>).
- Model changes preserve the current slot because model is absent from the reuse predicate at [registry.rs:503](</private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo/crates/bridge-registry/src/registry.rs:503>).
- `configure_session(None)` stores `None`, while `resolve_model` cannot distinguish that from no override and falls back to the old backend configuration at [backend.rs:258](</private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo/crates/bridge-api/src/backend.rs:258>) and [backend.rs:687](</private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo/crates/bridge-api/src/backend.rs:687>).

Conditions and likelihood: **plausible**. It requires an already-warm API agent followed by a model-removal reload; both are supported production configurations.

Exposure and impact: API inline, dispatcher, preflight-real-turn, retry, resume, and batch paths. The impact is wrong provider selection plus false persisted provenance and potentially different billing/output.

Bounded fix: make API session model state tri-valued—unconfigured versus explicitly `None` versus `Some(model)`—so an explicit bound `None` suppresses the stale backend default. Add `bridge-api` to integration ownership. Alternatively, API model changes must force a fresh slot, but that conditional must be normative.

Cost/blast radius: **low-to-medium**; API session state, registry/integration ownership, and focused tests.

Exact fail-first regression: warm an API backend under `M`, reload to `None`, freeze and bind a new attempt, and assert the request does not contain `M`. Negative cases must prove `M→B` sends `B` and an unconfigured legacy session may still use its spawn default.

Disposition: **BLOCKER**. Plausible reachability × high provider/provenance impact × bounded repair.

### WRONG W1-B — BLOCKER: the persisted effect digest is a credential-verification oracle

Constructible state: configure an MCP environment credential with a four-digit value while all other entry fields are public. The plan hashes the exact MCP environment value using deterministic unkeyed SHA-256 and persists the result. An artifact reader enumerates 10,000 candidate values using the public canonical encoding and recovers the credential, even though the raw bytes do not literally appear in the artifact.

Mechanism and location:

- MCP environment values are literal delivered values and are treated as credential material by current redaction at [mcp.rs:22](</private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo/crates/bridge-core/src/mcp.rs:22>) and [mcp.rs:83](</private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo/crates/bridge-core/src/mcp.rs:83>).
- The design includes their exact values in a durable unkeyed SHA-256 digest at [focused boundary:564](</private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo/docs/superpowers/plans/2026-08-01-r2f1a-focused-boundary.md:564>).
- Its non-disclosure fixture checks only absence of raw bytes at [focused boundary:1597](</private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo/docs/superpowers/plans/2026-08-01-r2f1a-focused-boundary.md:1597>), which does not detect offline guessing.

Conditions and likelihood: **rare**—requires a literal low-entropy credential, artifact access, and knowledge of the remaining configuration. All are constructible under the supported schema.

Exposure and impact: every persisted snapshot/history/projection containing `effect_digest`; credential and custody impact.

Bounded fix: use a domain-separated HMAC under a stable installation/ledger identity key stored outside projected artifacts. Resume without that key must refuse before effects. Alternatively, change MCP credential configuration to indirect variable names plus a nonsecret rotation/version identity.

Cost/blast radius: **medium**; digest construction, durable key custody, resume, and non-disclosure tests.

Exact fail-first regression: persist a digest for a credential selected from `0000..9999` and demonstrate recovery through enumeration. The repaired artifact must provide no offline verifier without the separately held key, while value changes still cause effect drift.

Disposition: **BLOCKER**. Rare reachability is outweighed by credential exposure and the explicit non-disclosure contract.

### WRONG W3 — BLOCKER: configured-store accounting remains nonconservative

Constructible state: use a configured shared store with 512-byte pages, retained/pinned history near its 128-MiB charged ceiling, and reserve an attempt containing a 1-MiB node ID plus its placeholder. The logical comparison admits based on exact ID bytes plus 256 bytes. SQLite stores only a reduced local payload and uses 508-byte overflow payload pages; the overflow pointers alone exceed 8 KiB, before record, B-tree, and WAL overhead. The charged allocation can therefore remain at or below 128 MiB while the workflow-history rows and their WAL reserve exceed it.

Mechanism and location:

- The equation claims a fixed 256 bytes bounds record and B-tree overhead at [focused boundary:1242](</private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo/docs/superpowers/plans/2026-08-01-r2f1a-focused-boundary.md:1242>).
- Configured stores explicitly receive no physical gate at [focused boundary:1373](</private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo/docs/superpowers/plans/2026-08-01-r2f1a-focused-boundary.md:1373>).
- Current `history_growth_fits` returns `true` unconditionally outside platform allocations at [sqlite.rs:1959](</private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo/crates/bridge-store/src/sqlite.rs:1959>).
- The owner contract requires a conservative account covering rows and their WAL reserve at [owner design:337](</private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo/docs/superpowers/specs/2026-07-20-r2f-owner-design.md:337>).
- Bundled SQLite confirms variable local payload and `usable_size-4` overflow pages at [sqlite3.c:73956](</Users/wesleyjinks/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libsqlite3-sys-0.30.1/sqlite3/sqlite3.c:73956>) and [sqlite3.c:81421](</Users/wesleyjinks/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libsqlite3-sys-0.30.1/sqlite3/sqlite3.c:81421>).

Conditions and likelihood: **rare** for a 1-MiB ID near capacity, but smaller undercounts also occur with many ordinary nodes and live WAL frames.

Exposure and impact: configured durable stores, migration, retention credits, disk custody, and later `SQLITE_FULL`/I/O behavior.

Bounded fix: remove the fixed-overhead assertion. Debit a measured table-local materialized-page charge plus a conservative measured reserve for every remaining bounded history WAL mutation, under the serialized pre-effect transaction; roll back if the configured history allocation would exceed 128 MiB. Unrelated primary tables remain excluded, and no ID cap, truncation, or hash substitute is permitted.

Cost/blast radius: **medium-to-high** across configured accounting V2, migration, collection, and shared-WAL fixtures.

Exact fail-first regression: in an otherwise empty 512-byte configured store, materialize a 1-MiB-ID placeholder and compare the proposed debit with history-table pages plus attributable WAL reserve; then seed allocation to `MAX_CHARGED_BYTES - proposed_charge` and prove the current equation admits an over-cap state. Repeat with many short IDs and a pinned WAL reader.

Disposition: **BLOCKER**. It leaves inherited W3 incomplete; rare boundary reachability × hard custody invariant × bounded storage repair warrants correction.

### WRONG W5 — BLOCKER: the required overflow comparison cannot pass the specified fallback

Constructible state: build a nonoverflowing terminal with `dependency_set=Some(...)`, then force the overflow fallback as required by the fault-injection fixture. The fallback unconditionally drops `dependency_set`; if needed it also shortens `deepest_cause` and changes `cause_truncated`. The mandatory test nevertheless requires `evidence_overflow` to be the only field differing from the nonoverflowing counterpart.

Mechanism and location:

- The fallback is required to drop dependency evidence and may alter cause evidence at [focused boundary:462](</private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo/docs/superpowers/plans/2026-08-01-r2f1a-focused-boundary.md:462>).
- The acceptance matrix requires the overflow flag to be the only difference at [focused boundary:1683](</private/tmp/a2a-bridge-r2f1a-round3.jh3cr3/opus-repo/docs/superpowers/plans/2026-08-01-r2f1a-focused-boundary.md:1683>).

Conditions and likelihood: runtime overflow is **theoretical** under the current 1,978-byte proof, but execution of the required fault-injection regression is **common/certain** during implementation verification.

Exposure and impact: encoder acceptance and evidence gates. The primary production evidence-preservation mechanism is otherwise sound, but no implementation can satisfy both mandatory assertions.

Bounded fix: state that `evidence_overflow` is the only dedicated overflow-classification field, not the only serialized difference. Explicitly permit only `dependency_set`, `deepest_cause`, and `cause_truncated` to change in addition to the flag; class, code, primary, cleanup, acceptance, ancestry, and trigger must remain equal.

Cost/blast radius: **very low**; specification wording and one comparison fixture.

Exact fail-first regression: force fallback from a terminal with a populated dependency set and assert the precise allowed-difference set. The current “flag only” predicate must fail; class/code substitution or loss of required causal fields must also fail.

Disposition: **BLOCKER**. The production trigger is theoretical, but the mandatory gate is deterministically impossible and the repair is trivial.

## SMELL findings

None.

## Defect population and readiness

The remaining population is **closed-enumerable**, not open-class: API tri-state model use, credential-safe effect commitment, configured-store conservative accounting, and one encoder-test contradiction each have a bounded repair. This was the final authorized design round; the cap is exhausted, so the checkpoint must be parked and escalated rather than silently receiving another repair/review turn.

Measured this turn: exact cwd, clean status, HEAD/parent, both artifact digests, complete parent diff, and bundled SQLite version/source formulas.

Observed in source: registry publication and slot reuse ordering, API model fallback, literal MCP credential delivery/redaction, platform-only physical gating, all collision producers, and roadmap custody.

Specified but not exercised: the new bound registry APIs, V2 schema/migration, placeholder transactions, classifier, encoder, projections, and implementation stages.

Not exercised by mandate: edits, builds, tests, SQLite runtime probes, migrations, fault injection, provider/network activity, releases, deployment, or live operator behavior. No green implementation evidence exists or is claimed.

VERDICT: REJECT
SUMMARY: Four closed-enumerable blockers remain: stale warm-API model use, credential-verifier leakage, nonconservative configured-store accounting, and an impossible overflow regression.
