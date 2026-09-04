# R2f1b slice 4H-2 repair handoff

## Public merge closure and 4I residual census (current)

Custody was rebound on 2026-09-04 in isolated branch `docs/r2f1b-4i-census-20260904` from freshly fetched
`origin/main` `52b05d70f14fc1080707fde1de4e9818a9d81d0f`. PR #89 publicly merged 4H-2 as
`54529b1d83a9fbe97d400cded02dcfbdf69683e3`, sole parent `fd1f66f253c0f5128fed438f96b79dfabadc4d2f`,
tree `1baaccba6b5a41f53411a422678b0421ca3d8cfd`. PR #90 then advanced `main` to the measured head above without
changing the 4H-2 reliability obligation. The publication gate described below is closed; the historical local
landing remains provenance rather than current operating direction.

**4I census verdict: RESIDUAL CONFIRMED / IMPLEMENTATION NOT AUTHORIZED.** `WRONG`: in the active fail-fast state,
an early failed root can pass its durable trigger barrier and cancel a running sibling. If that sibling's node
future does not resolve after cleanup-deadline transfer, the handler at the scheduler cleanup deadline records the
transfer but neither removes nor terminalizes the sibling. The loop's normal break still requires `inflight` to be
empty. The incorrect result for that concrete state is an absent sibling terminal, absent downstream
not-started-by-policy terminal, and absent workflow terminal instead of bounded failed terminalization. This is the
workflow-scheduler mechanism reported by issue #22; it is not an adapter-only inference.

The exact current-main 4H-2 module passed **11 / 0 / 152 filtered** on 2026-09-04. Source and test census found
barrier, cancellation, exhaustion, first-cause, and parking coverage, but every blocked sibling is eventually
released; no test retains a sibling beyond cleanup transfer and requires complete terminal projection. That green
module therefore proves preserved 4H-2 mux behavior, not 4I closure. The bounded implementation contract is
[`2026-09-04-r2f1b-slice4i-task.md`](../plans/2026-09-04-r2f1b-slice4i-task.md). A genuine current-main RED through
the production executor path is the first implementation gate. No 4I Rust edit, provider turn, live smoke,
compatibility execution, release, deployment, or operator mutation occurred in this census.

Repository-wide readiness remains `Disarmed`. 4J is a separate, minimal, independently revertable arming decision
only after an implemented and reviewed 4I discharge. The former raw evidence path
`/private/tmp/a2a-r2f1b-4h2-am8-compat-diagnostic` is now absent; only its historical hashes below remain, and this
handoff no longer describes the disposable path as current custody.

## Amendment 8 approved and locally landed (historical; superseded by public merge)

Landing custody: the owner approved continuation after the terminal Amendment 8 review. The controller fetched exact remote `main` `fd1f66f253c0f5128fed438f96b79dfabadc4d2f`, created local branch `feat/r2f1b-4h2-multiplexer` at that SHA, preserved the original implementation checkpoint at SHA-256 `de68d8b0deb80bbb768e2483ce8ad2c447ee0cfd69f8e68da3751f8f4bb9a8c1`, rebound only its stale base/current custody pointers, and invoked the repository's exact-base merge workflow once. Operator-authored code landing commit `b541e2ad8f04ae50a9fd9c782eaabe8c8b8e3826` has sole parent `fd1f66f2` and tree `5695b42a57ba5f29166189d754c6b50c3d57215f`, exactly equal to reviewed post-handoff candidate `a98846da`. Existing checked-out `main` `cafeae13` and controller `4701acd9` remained unchanged. The bridge landed successfully but initially could not reap intentionally unreadable test-fixture directories; retained run metadata was already archived at SHA-256 `0af87008a626a7420d882b8a668a9d2b2451f0300a810e9f663b17a36cdf235f`, owner traversal was restored only on that exact clone, and the clone was then removed. This docs-only successor changes no Rust bytes. No GitHub push, pull request, R3 work, release, deployment, live smoke, billable compatibility execution, or running-operator mutation occurred.

Owner authority: the owner authorized one separately bounded aggregate-test reliability repair and one later Sol/xhigh hard-read-only cumulative review. Tier-3 writer execution `exec-612bee48758ecacaed804dfc76fcb7eb` / attempt `attempt-4c2c222aace0a06651fe0856e11106aa` consumed the sole provider-contacting writer slot and staged exactly the two authorized paths. The controller then committed exact code candidate `c9a9b5a433b1e201c4ba1b0312925f193a37bc4a`. Sol/xhigh review execution `exec-8c3e61eb3098215edbf6cc14c8a07edc` / attempt `attempt-fb1761a1a0f271114bee33307ba51772` consumed the sole review slot and returned `VERDICT: APPROVE`. Neither provider turn committed, switched branches, reset, restored, rebased, fetched, pushed, merged, ran live smoke or a billable compatibility case, deleted caches, restarted, invoked `implement --resume`, or mutated a running operator.

Incoming custody was verified before editing: clean branch `implement/impl-15417-3tup610h`, HEAD `6e49b4bb814d867824b0cd1e94c7703422ce4672`, exact code parent `f0a8d21fe510f9c3ff49aa048b377e4150b8ca38`, grandparent `1ba4d7ed0f02118da795fbef4c87b7f8fc484cfb`, and Approved R1 `f3c4c2b341902bb70920c0163221c3cfafa80f1f` as an ancestor. Index and worktree were empty. The controller authority is `6b511dbbcc3a2f0f4c221475ca91108651dda287`; its roadmap, task, and this retained handoff bind the Amendment 8 scope. The code candidate's sole parent is `6e49b4bb`. Independent verification was recorded in handoff-only commit `51052230050d7ac04bfe6cfd4b1421a1fa9d099a`, whose sole parent is `c9a9b5a4`; the Sol/xhigh reviewer verified that exact clean custody and linear ancestry before review. This post-review handoff-only successor preserves the exact `c9a9b5a4` Rust tree.

Closed mechanism and supplied pre-change RED: the configured aggregate intermittently failed Linux staged-candidate process tests with `Os { code: 26, kind: ExecutableFileBusy, message: "Text file busy" }`. Exact isolated candidate and parent tests passed. A disposable exact-`f0a8d21f` timing magnifier held the first candidate creation writer while a sibling child waited in `pre_exec`; one of three parallel `staged_candidate_*` tests then failed with `ETXTBSY`. The `/proc` census found the failing inode in multiple processes and found a different child holding fd 25 with access mode `O_WRONLY` (`fdinfo` flags ending in `0001`). This proves `O_CLOEXEC` does not prevent the forked child from retaining the writer until exec, so a sibling candidate exec in that interval can be rejected. Same-test overwrite and raw-fd reuse were falsified by the read-only retained candidate descriptor and the different child owning the writable same-inode fd. The diagnostic source/log custody path `/private/tmp/a2a-r2f1b-4h2-am8-compat-diagnostic` was host-only and not mounted here.

Implementation summary: the repair is test-only in `bin/a2a-bridge/src/compatibility.rs`. Before any staged candidate writer is opened, each of the four staged-candidate process tests re-execs the already-built current test binary with a child-only `A2A_BRIDGE_ISOLATED_STAGED_CANDIDATE_TEST` marker and an exact filter for itself, then the outer test returns. Only the marked child executes the existing staged-candidate body. Child output is captured with `Command::output`; successful children do not print nested `test result` summaries into the parent aggregate. Setup failure, zero/wrong selection, signal, or nonzero status panics the outer test with bounded stdout/stderr/status evidence. The helper does not mutate the parent process environment. A new negative regression, `staged_candidate_isolated_child_failure_propagates_to_outer_failure`, intentionally fails only in the marked child and uses outer `catch_unwind` to prove the child failure is observed rather than swallowed as a zero-tests-selected success. No production code, scheduler code, `stage_candidate`, `ProcessSmokeInvoker`, candidate digest/object custody, compatibility run behavior, or retry/serialization behavior was changed.

Changed paths for Amendment 8 are exactly:

- `bin/a2a-bridge/src/compatibility.rs`
- `docs/superpowers/reviews/2026-08-24-r2f1b-slice4h2-handoff.md`

Focused local test attempt:

```bash
env CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo test -p a2a-bridge --bin a2a-bridge staged_candidate_ -- --test-threads=1
```

Result: setup-blocked before compilation or harness selection because the local Cargo registry cache is incomplete: `error: no matching package named 'arc-swap' found`. A prior default online Cargo probe also stopped before compilation because the CONNECT tunnel returned HTTP 403 while fetching `a2a-lf`. Therefore no trustworthy local selected/pass/fail counts were emitted in this retained writer environment; the controller replay owns the focused five-test count and the timing-magnifier replay.

Formatting and whitespace gates:

```bash
cargo fmt --all -- --check
```

Result: GREEN, no output, after running `cargo fmt --all` once to apply the two rustfmt line wraps and immediately staging the reformatted Rust file.

```bash
git diff --check
git diff --cached --check
```

Result: GREEN, no output.

Line caps and path identity: Amendment 8 adds **88 nonblank formatted Rust lines / 120** against exact parent `f0a8d21fe510f9c3ff49aa048b377e4150b8ca38`. Exact Amendment 8 Rust path set: `bin/a2a-bridge/src/compatibility.rs`. The scheduler path `crates/bridge-workflow/src/executor.rs` is byte-identical to incoming parent `f0a8d21fe510f9c3ff49aa048b377e4150b8ca38` and remains exactly **396 / 400** added nonblank Rust lines against Approved R1 `f3c4c2b341902bb70920c0163221c3cfafa80f1f`.

Independent controller verification used immutable image `sha256:bb09479fd020b22313782830d3a640e1b97be4a5a0ecd70e46fb5985f4ff3086`, cache `a2a-verify-cache-8ac4ca8ed1db0dde`, and the exact retained checkout path. The focused staged-candidate population is GREEN: **5 passed / 0 failed / 1,093 filtered**, with exactly one parent summary and no nested child summary; log SHA-256 `fd0febc4a181cbfb686768ecbe330c08bbbb34c6d56d54fd35bde037bf673c2f`. The pre-change timing magnifier is the discriminating RED: **2 passed / 1 failed / 1,094 filtered**, `ETXTBSY`, and a different child's same-inode `O_WRONLY` holder; SHA-256 `8124484c543aacae3a29ebe7190191693ad1d08a9cda3ec44c99406755b64dab`. The same repaired magnifier is GREEN: **5 / 0 / 1,093**, no diagnostic status or inode-holder marker, SHA-256 `353023dfe0f9fed0044758bba4db5fd2a1d4d3db74c350f8cfe558c8c1fa0701`.

Formatter, warnings-denied all-target/all-feature locked Clippy, locked build, and candidate-built repository hygiene are GREEN; hygiene reports **40 tracked artifacts / 8 validated example configs**. The preserved 4H-2 module is GREEN: **11 passed / 0 failed / 152 filtered**. The first aggregate command is inadmissible as a complete suite because 15 doc-test targets were never executed when scoped Cargo could not resolve bare `rustdoc`; its ordinary binary targets, including all five staged-candidate tests and the main binary's **1,098 / 0**, were green. Binding `RUSTDOC` to the installed image toolchain repaired only that probe setup. The corrected configured selection exited 0 with **99 summaries / 4,286 passed / 0 failed / 12 ignored / 0 measured / 716 filtered**; log SHA-256 `0c5771ba4ae7ccd9a0ea4f0156edf47f06cc022cfecb9268689a6232ba9ac441`. No same-environment parent attribution was required because the only red aggregate observation was a never-executed tool-path setup failure, not candidate behavior.

Task input SHA-256 is `18c19a8bd96327b7673554e783cc2207fdad8efe6e932ef24460ae81b30ed5b4`; writer output SHA-256 is `fb8e03960335e825c21f8b7750c6d29fd432b2203822da57ba03544cb5f0989a`. Two preceding run-workflow admissions were refused before agent spawn/provider prompt: one because the internal edit prompt did not consume `{{input}}`, and one because the pre-existing config lacked the required provider-effect commitment key for its MCP environment. They are inadmissible as writer evidence and did not spend the writer slot. The private keyed config changed only `[security].provider_effect_key_file`; model/image/MCP/write-boundary bytes were unchanged.

The sole `gpt-5.6-sol` / `xhigh` / hard-read-only cumulative review inspected exact clean review custody `51052230050d7ac04bfe6cfd4b1421a1fa9d099a`, the complete `f3c4c2b3..51052230` cumulative diff, Amendment 8 code `6e49b4bb..c9a9b5a4`, and the handoff-only `c9a9b5a4..51052230` delta. It established no `WRONG` finding and returned `VERDICT: APPROVE`. The reviewer independently closed the inherited Amendment 6 `SMELL / BLOCKER`: Amendment 7's `true` test override reaches the active acknowledgement site, `false` remains passive, both overrides are `cfg(test)`, and the retained site-specific mutations discriminate those production boundaries. It also accepted Amendment 8's normal isolation mechanism: all four outer tests re-exec before opening a staged writer, exact selection and nonzero/signal/setup failures remain visible, recursion is prevented, the negative child failure propagates, and no production path changed.

Nine nonblocking `SMELL / DEFER` items remain: the prior seven (production select-priority mutation evidence; cleanup-guard retention after projection; custody on transfer errors; bounded preservation after deadline; historical-versus-live owner selection; protected `ApiBackend` transfer; repository-local authority custody), plus an externally pre-set exact isolation marker that can bypass outer re-exec, and `Command::output()` buffering complete child streams before display truncation. The latter two are rare test-harness risks; neither establishes an incorrect current result or blocks this candidate. Review input SHA-256 is `481135d2fe416fce4866d32d417277d17d0d260f67616e3d3415e12687f5bc50`; output SHA-256 is `a307dba21cf0528fcf6bc06e61337e5e364ab40478fbc1d408f091d96179b5ca`; strict brief-lint SHA-256 is `1440a54c767bb6c1168c6ca62ffb735c6da172e5621c3d754a4a8b24e3ab3f8a`.

**Current disposition: R1 APPROVED / R2 AMENDMENT 8 APPROVED / LOCALLY LANDED / PUSH AND R3 NOT AUTHORIZED.** Implementation, verification, review, and exact-base local landing are complete at code commit `b541e2ad8f04ae50a9fd9c782eaabe8c8b8e3826`; this docs-only successor updates custody without changing the reviewed Rust tree. No additional repair/review cycle, GitHub push or pull request, R3 work, release, deployment, live smoke, billable compatibility execution, or running-operator mutation is authorized by this handoff.

## Amendment 7 identity repair retained; controller full-suite gate parked before review (historical, superseded by amendment 8)

Owner authority: the owner explicitly authorized one bounded Amendment 7 Tier-3 retained-artifact repair and one later Sol/xhigh hard-read-only cumulative review. This write turn consumed the sole authorized writer slot. No commit, branch switch, reset, restore, rebase, fetch, push, merge, live smoke, compatibility case, cache deletion, restart, `implement --resume`, or running-operator mutation was performed.

Incoming custody was verified before editing: clean branch `implement/impl-15417-3tup610h`, HEAD `1ba4d7ed0f02118da795fbef4c87b7f8fc484cfb`, Amendment 6 terminal parent `9139fbd982682831ee8e106cfcdf1f587c2fbb0f` as ancestor, exact production-code candidate `af423b4cf364aaf8d70784295f7757647900333c` as ancestor, and Approved R1 `f3c4c2b341902bb70920c0163221c3cfafa80f1f` as ancestor. Index and worktree were empty. The mounted Amendment 6 review artifact `/evidence/amendment6/amendment6-review-output.md` was verified at SHA-256 `3f7abef51f11d8aa2a0fabb07aefc772339c73e43925a6ec4029f15daffa0ed0`.

Pre-repair source inspection confirmed the active identity gap: `PostRecordingExitFaultForTest::AcknowledgementExternalCancel` had no activation identity; the test-only `scheduler_active` override recognized only `SchedulerArmed(_)`; both acknowledgement regressions passed the same cancellation fault; and repository-wide `scheduler_activation_readiness_v1()` remained `Disarmed` under the frozen `ManualOnlyR2f1a` run specs. No other inspected input made the named active acknowledgement test effectively active.

Pre-repair active-site-only mutation control: only the active acknowledgement site's value-taking `observe_external_root_cancellation!(acknowledgement_cancelled)` was temporarily changed to `observe_external_root_cancellation!(false)`, leaving `FanOutControllerV1::acknowledge_barrier(barrier, acknowledgement_cancelled)` unchanged. The exact active regression survived, proving the unrepaired fixture did not traverse the active site:

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo test -p bridge-workflow executor::tests::slice4h2_mux::active_acknowledgement_external_cancel_records_workflow_provenance -- --exact
```

Pre-repair mutation-control total: selected 1 unit test; 1 passed, 0 failed, 0 ignored, 0 measured, 162 filtered. The mutation was then reversed exactly and `git diff --name-only` plus `git status --porcelain=v1` were empty before implementation.

Implementation summary: the repair is test-only identity plumbing in `crates/bridge-workflow/src/executor.rs`. `AcknowledgementExternalCancel` now carries a boolean activation identity. The hook still cancels at both acknowledgement sites for either boolean value. The test-only `scheduler_active` override preserves `SchedulerArmed(_)` semantics and additionally recognizes only `AcknowledgementExternalCancel(true)`. The passive regression passes `AcknowledgementExternalCancel(false)` and the active regression passes `AcknowledgementExternalCancel(true)`. No non-`cfg(test)` production behavior was intentionally changed, and both production acknowledgement boundaries retain the same immutable cancellation sample feeding first-cause observation and `acknowledge_barrier`.

Post-repair focused GREEN evidence:

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo test -p bridge-workflow executor::tests::slice4h2_mux::passive_acknowledgement_external_cancel_records_workflow_provenance -- --exact
```

Passive regression total: selected 1 unit test; 1 passed, 0 failed, 0 ignored, 0 measured, 162 filtered.

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo test -p bridge-workflow executor::tests::slice4h2_mux::active_acknowledgement_external_cancel_records_workflow_provenance -- --exact
```

Active regression total: selected 1 unit test; 1 passed, 0 failed, 0 ignored, 0 measured, 162 filtered.

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo test -p bridge-workflow executor::tests::slice4h2_mux::policy_global_drain_keeps_unscheduled_node_policy_stopped -- --exact
```

Policy-only negative total: selected 1 unit test; 1 passed, 0 failed, 0 ignored, 0 measured, 162 filtered.

Post-repair active-site mutation RED and passive cross-path GREEN: the active acknowledgement site's value-taking observation was temporarily changed to `false`, with acknowledgement still using the sampled boolean. The active regression selected exactly one unit test and failed behaviorally on primary provenance: `left: CanceledPolicy`, `right: CanceledWorkflow`. With that same mutation present, the passive regression selected exactly one unit test and passed.

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo test -p bridge-workflow executor::tests::slice4h2_mux::active_acknowledgement_external_cancel_records_workflow_provenance -- --exact
```

Active-site mutation RED total: selected 1 unit test; 0 passed, 1 failed, 0 ignored, 0 measured, 162 filtered.

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo test -p bridge-workflow executor::tests::slice4h2_mux::passive_acknowledgement_external_cancel_records_workflow_provenance -- --exact
```

Passive cross-path total: selected 1 unit test; 1 passed, 0 failed, 0 ignored, 0 measured, 162 filtered.

Post-repair passive-site mutation RED and active cross-path GREEN: the passive inline acknowledgement site's value-taking observation was temporarily changed to `false`, with acknowledgement still using the sampled boolean. The passive regression selected exactly one unit test and failed behaviorally on policy primary provenance: `left: NotStartedPolicy`, `right: CanceledWorkflow`. With that same mutation present, the active regression selected exactly one unit test and passed.

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo test -p bridge-workflow executor::tests::slice4h2_mux::passive_acknowledgement_external_cancel_records_workflow_provenance -- --exact
```

Passive-site mutation RED total: selected 1 unit test; 0 passed, 1 failed, 0 ignored, 0 measured, 162 filtered.

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo test -p bridge-workflow executor::tests::slice4h2_mux::active_acknowledgement_external_cancel_records_workflow_provenance -- --exact
```

Active cross-path total: selected 1 unit test; 1 passed, 0 failed, 0 ignored, 0 measured, 162 filtered.

After restoring both mutation controls, the exact passive regression, active regression, and policy-only negative were rerun and each selected 1 unit test with 1 passed, 0 failed, 0 ignored, 0 measured, 162 filtered.

Complete Slice 4H-2 module evidence:

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo test -p bridge-workflow executor::tests::slice4h2_mux::
```

Complete module total: selected 11 unit tests; 11 passed, 0 failed, 0 ignored, 0 measured, 152 filtered.

Formatting and whitespace gates:

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo fmt --all -- --check
```

The required wrapper form was attempted first and failed before formatting because `cargo` under `CARGO_HOME=/cache/cargo` could not locate `cargo-fmt` (`error: no such command: fmt`). Installed components were then located at `/usr/local/rustup/toolchains/1.94.0-aarch64-unknown-linux-gnu/bin/rustfmt` and `/usr/local/rustup/toolchains/1.94.0-aarch64-unknown-linux-gnu/bin/cargo-fmt`; no installation or fetch was performed. The check was rerun with only that exact toolchain bin exposed in scoped PATH:

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true PATH=/usr/local/rustup/toolchains/1.94.0-aarch64-unknown-linux-gnu/bin:$PATH cargo fmt --all -- --check
```

Result: GREEN, no output.

```bash
git diff --check
```

Result: GREEN, no output.

Line cap: cumulative added nonblank formatted Rust lines against exact Approved R1 `f3c4c2b341902bb70920c0163221c3cfafa80f1f` measured **396 / 400**. Exact Rust path set: `crates/bridge-workflow/src/executor.rs`. Raw `git diff --numstat` for Rust against R1 was `414	51	crates/bridge-workflow/src/executor.rs`; the cap count excludes blank added Rust lines.

Controller custody and independent focused verification are complete. The exact retained code candidate is commit `f0a8d21fe510f9c3ff49aa048b377e4150b8ca38`, parent `1ba4d7ed0f02118da795fbef4c87b7f8fc484cfb`, on branch `implement/impl-15417-3tup610h`. The worktree was clean after that commit. The controller used immutable Tier-3 image `sha256:bb09479fd020b22313782830d3a640e1b97be4a5a0ecd70e46fb5985f4ff3086` and warmed volume `a2a-verify-cache-8ac4ca8ed1db0dde`. Formatting, warnings-denied all-target/all-feature locked Clippy, locked workspace build, and candidate-built repository hygiene all passed; hygiene reported **40 tracked artifacts / 8 validated example configs**. The passive regression, active regression, and policy-only negative each selected **1 / 1 passed**; the complete Slice 4H-2 module passed **11 / 11**. The controller independently repeated the pre-repair surviving active-site mutation and both repaired-site mutation controls: the unrepaired active regression survived its active-site-only mutation, while the repaired active and passive regressions each failed on their intended production-site mutation and their opposite-path controls stayed green.

The configured workspace gate is not green. Its exact selection remained:

```bash
cargo test --workspace --locked --no-fail-fast --quiet --exclude bridge-container -- --skip process::tests::terminate_reaps_child_no_zombie --skip process::tests::term_ignoring_loop_forces_group_sigkill --skip process::tests::drop_group_kills_descendants
```

The first admissible candidate aggregate completed **99 summaries / 4,282 passed / 2 failed / 12 ignored / 0 measured / 716 filtered**. The failures were `compatibility::tests::staged_candidate_cannot_be_overwritten_in_place_after_digest_check` and `compatibility::tests::staged_candidate_nonzero_exit_retains_process_status`. Its retained log is `/private/tmp/a2a-r2f1b-4h2-am7.1TbfIu/amendment7-candidate-full-test.log`, SHA-256 `83d6a308c4c71002d2bf41f338bf198af047436d5663ba52d532206634afd7f5`. Each exact candidate rerun passed **1 / 1**, and each exact incoming-parent `1ba4d7ed0f02118da795fbef4c87b7f8fc484cfb` rerun passed **1 / 1** in the same image/toolchain/cache setup. Those controls show the aggregate failures are not deterministic and do not establish Amendment 7 causality.

Two probe failures were inadmissible and caused no belief update: one aggregate never reached a test because the corrected toolchain PATH still lacked `rustdoc`; another reused test artifacts with a mismatched compile-time source path. A later candidate aggregate invocation also mixed cached host-path artifacts with a `/work` source mount and omitted `/usr/local/cargo/bin`; its resulting path-not-found and missing-`rust-analyzer` population is inadmissible. It nevertheless repeated `staged_candidate_nonzero_exit_retains_process_status` inside the main binary test target. That log is `/private/tmp/a2a-r2f1b-4h2-am7.1TbfIu/amendment7-candidate-full-test-final.log`, SHA-256 `857ac650ac038f0f3404ad27a5fbca5be99ac4424bf0f7736f3eeeab2a8c1c18`.

The aggregate parent control completed **99 summaries / 4,284 passed / 1 failed / 12 ignored / 0 measured / 716 filtered**. It did not reproduce either compatibility failure; its sole failure, `config::tests::worktrees_config_parses_and_preflight`, came from the archive control lacking Git repository metadata, so the aggregate parent is not a clean full-suite control. Its log is `/private/tmp/a2a-r2f1b-4h2-am7.1TbfIu/amendment7-parent-full-test.log`, SHA-256 `95b15e0befc408f497505223d795298535a6d04e6e9e4c710d5bad53a82edc80`. The exact parent compatibility controls remain admissible and green, but candidate attribution is therefore a hypothesis rather than a finding.

Cap classification: the required aggregate gate repeatedly exposed the same open-class compatibility child-process/status family while exact candidate and exact-parent controls stayed green. Per convergence discipline, the controller parked the lane rather than extending the full-suite retry loop. The one authorized Sol/xhigh hard-read-only cumulative review was **not dispatched** because its all-gates-green prerequisite was false; no review provider turn was spent. Amendment 7's bounded identity repair remains retained, but R2f1b 4H-2 is **PARKED / NOT APPROVED**. A subsequent owner decision must either authorize a separately bounded aggregate-test reliability repair/design lane or explicitly rescope the acceptance gate before any review. R3, additional repair, restart, `implement --resume`, merge, push, live smoke, compatibility execution, release, deployment, and running-operator mutation remain unauthorized.

## Amendment 6 terminal review rejection (historical, superseded by amendment 7)

Owner authority: the owner explicitly authorized one Tier-3 continuation of the retained partial RED harness and one cumulative hard-read-only review for R2f1b 4H-2 amendment 6. Both authorized actions are complete. This section is historical evidence superseded by Amendment 7. Amendment 5's `VERDICT: REJECT` remains historical evidence and is superseded by this Amendment 6 terminal disposition.

Incoming custody was verified before editing: branch `implement/impl-15417-3tup610h`, HEAD `95a163df4f55daae01ed17d76c2cb57e4db157b4`, Approved R1 `f3c4c2b341902bb70920c0163221c3cfafa80f1f` as an ancestor, empty index, and exactly one modified path, `crates/bridge-workflow/src/executor.rs`. The incoming unstaged executor diff SHA-256 was `cc7dbcac99b7353a0db167261410340ca234d5de35067265e2cb4dc38711661b`; incoming `git diff --numstat -- crates/bridge-workflow/src/executor.rs` was `76	0	crates/bridge-workflow/src/executor.rs`. The mounted Amendment 5 re-review artifact was verified at `/evidence/amendment5/amendment5-rereview-output.md`, SHA-256 `6deefbfa0215deef6cfe52e8a3b1c35fce72b1fc28e223b50a35d52f7ab76603`.

Provider custody: the first Amendment 6 dispatch (`exec-edaddd9816a372b07e5f0a9fca80c9bb` / `attempt-beb4ba5ec32bc9d316ef2488210a7bb8`) stopped cleanly without edits because the review artifact was not mounted; its output SHA-256 is `c999de17d52964b87c9c8539a724c39ffae642d3819d6f149d267ed025f02c46`. The owner-authorized replacement (`exec-21a2193af446fd962cd16092651092dd` / `attempt-38495c6243fe3f28939b9e436ce9232f`) retained the 76-line RED harness and stopped before production correction when its Cargo cache was incomplete; output SHA-256 `347916b27ddce3fe515ad5aa24bba6bec1c8b493bac6a0800ac98bf56424ce47`. The owner-authorized continuation (`exec-397b400dc3be99eb906327a40db2d384` / `attempt-15ac1edb763af84a5ac7d0f01ffbe2a2`) used the controller-proven warmed cache, completed the repair and writer gates, and exited with joined cleanup; output SHA-256 `083814f3e569c9125ec3b7bfc9946e851ab36c88d93977b8e5561a31c7d0eab5`.

Implementation summary: the retained partial harness first received only the two mechanical borrow fixes, changing both test-only acknowledgement-cancel hook calls to pass `&cancel`. With the old double-read production ordering still intact, both exact regressions produced admissible behavioral REDs. After both REDs, the production repair was limited to the active and passive acknowledgement boundaries: each boundary now fires the test hook immediately before one immutable `cancel.is_cancelled()` sample, records external-root/running-node workflow provenance from that sample, and passes the same sampled boolean to `FanOutControllerV1::acknowledge_barrier`. Existing `observe_external_root_cancellation!()` call sites keep their old local sampling behavior through the zero-argument macro arm. The repair does not infer provenance from the arbitration result and keeps first-cause writes first-write-wins.

Admissible RED evidence, run with the required warmed cache prefix:

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo test -p bridge-workflow executor::tests::slice4h2_mux::passive_acknowledgement_external_cancel_records_workflow_provenance -- --exact
```

RED total: selected 1 unit test; 0 passed, 1 failed, 0 ignored, 0 measured, 162 filtered. Exact mismatch: `left: NotStartedPolicy`, `right: CanceledWorkflow`.

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo test -p bridge-workflow executor::tests::slice4h2_mux::active_acknowledgement_external_cancel_records_workflow_provenance -- --exact
```

RED total: selected 1 unit test; 0 passed, 1 failed, 0 ignored, 0 measured, 162 filtered. Exact mismatch: `left: CanceledPolicy`, `right: CanceledWorkflow`.

GREEN and control evidence:

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo test -p bridge-workflow executor::tests::slice4h2_mux::passive_acknowledgement_external_cancel_records_workflow_provenance -- --exact
```

GREEN total: selected 1 unit test; 1 passed, 0 failed, 0 ignored, 0 measured, 162 filtered.

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo test -p bridge-workflow executor::tests::slice4h2_mux::active_acknowledgement_external_cancel_records_workflow_provenance -- --exact
```

GREEN total: selected 1 unit test; 1 passed, 0 failed, 0 ignored, 0 measured, 162 filtered.

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo test -p bridge-workflow executor::tests::slice4h2_mux::policy_global_drain_keeps_unscheduled_node_policy_stopped -- --exact
```

Negative-control total: selected 1 unit test; 1 passed, 0 failed, 0 ignored, 0 measured, 162 filtered.

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo test -p bridge-workflow executor::tests::slice4h2_mux::
```

Complete slice-module total: selected 11 unit tests; 11 passed, 0 failed, 0 ignored, 0 measured, 152 filtered. This retains the prior passive/Disarmed and active/Armed identity assertions and the accepted scheduler obligations.

Formatting and whitespace gates:

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true cargo fmt --all -- --check
```

The required wrapper form was attempted first and failed before formatting because the active Cargo wrapper could not locate `cargo-fmt` under `CARGO_HOME=/cache/cargo` (`error: no such command: fmt`). `cargo-fmt` was present at `/usr/local/rustup/toolchains/1.94.0-aarch64-unknown-linux-gnu/bin/cargo-fmt`, so the same check was run with the required prefix and a scoped PATH exposing that installed formatter:

```bash
env CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_NET_OFFLINE=true PATH=/usr/local/rustup/toolchains/1.94.0-aarch64-unknown-linux-gnu/bin:/usr/local/bin:/usr/bin:/bin cargo fmt --all -- --check
```

Result: GREEN, no output.

```bash
git diff --check
```

Result: GREEN, no output.

Line cap: cumulative added nonblank Rust lines in `crates/bridge-workflow/src/executor.rs` versus exact Approved R1 `f3c4c2b341902bb70920c0163221c3cfafa80f1f` measured **394 / 400** after formatting check. The controller independently recomputed the same count after the custody commit; the Rust path set against R1 was exactly `crates/bridge-workflow/src/executor.rs`.

The writer performed no commit, branch switch, reset, restore, rebase, fetch, push, merge, live smoke, compatibility case, cache deletion, or running-operator mutation. The controller created code-candidate custody commit `af423b4cf364aaf8d70784295f7757647900333c`, then independently verified its read-only source mount in immutable image `sha256:bb09479fd020b22313782830d3a640e1b97be4a5a0ecd70e46fb5985f4ff3086` with warmed volume `a2a-verify-cache-8ac4ca8ed1db0dde`. `cargo fmt --all -- --check`, warnings-denied `cargo clippy --all-targets --all-features --locked`, `cargo build --locked`, and the candidate-built repository-hygiene command all passed; hygiene reported **40 tracked artifacts / 8 validated example configs**. The passive regression, active regression, and policy-only negative control each selected **1 / 1 passed**, and the complete Slice 4H-2 module passed **11 / 11**. The configured workspace selection completed **99 summaries / 4,285 passed / 0 failed / 12 ignored / 0 measured / 716 filtered**. Its declared exclusions were the entire `bridge-container` package and exactly `process::tests::terminate_reaps_child_no_zombie`, `process::tests::term_ignoring_loop_forces_group_sigkill`, and `process::tests::drop_group_kills_descendants`; no same-environment parent control was needed because every candidate gate was green.

The one authorized host-side `gpt-5.6-sol` / `xhigh` / hard-read-only cumulative review completed with clean custody and joined terminal cleanup: execution `exec-24be80023e3a4e5eec2a75bf906738db`, attempt `attempt-86e29a3f4698112c9056e277a8a6e207`. The 14,386-byte output is `/private/tmp/a2a-r2f1b-4h2-am6.qRp3st/amendment6-review-output.md`, SHA-256 `3f7abef51f11d8aa2a0fabb07aefc772339c73e43925a6ec4029f15daffa0ed0`, verdict `REJECT`. The reviewer independently confirmed that both and only both production `acknowledge_barrier` calls use one immutable cancellation sample for provenance and arbitration, later first-write-wins writers preserve that decision, and cancellation after the sample is coherently ordered after acknowledgement. Thus the inherited production `WRONG / BLOCKER` is closed.

The rejection is one new `SMELL / BLOCKER` in acceptance evidence: `active_acknowledgement_external_cancel_records_workflow_provenance` freezes an Armed specification, but repository-wide readiness still returns `Disarmed`, and the test-only `scheduler_active` override recognizes only `PostRecordingExitFaultForTest::SchedulerArmed(_)`, not `AcknowledgementExternalCancel`. The named active regression therefore deterministically executes the passive inline acknowledgement path; reverting only the active call site would leave it green. The bounded future repair is test-only: encode activation identity in the acknowledgement-cancel fault, recognize the active value in the test override, and prove active-only and passive-only mutations discriminate their respective sites. The seven prior R3 concerns remain `SMELL / DEFER`. Prism/LSP were preflighted but unavailable inside the review turn, so the reviewer used bounded repository search and direct source tracing and disclosed that limitation.

The one-review cap is exhausted. Candidate custody `df794a2ce99f699c9e0d402fb3022237727ad5fd` is parked at `REJECT`: no further repair, review, restart, R3 work, merge, push, live smoke, compatibility execution, or running-operator mutation is authorized. Production readiness remains `Disarmed`.

---

## Amendment 5 terminal re-review rejection and R2 park (historical, superseded by amendment 6)

Owner authority: amendment 5 superseded amendment 4's terminal park with the explicit direction "fold fix then re-review". This section is historical and superseded by Amendment 6 above; Amendment 7 is the sole current handoff section for the retained clone.

Incoming custody was clean on branch `implement/impl-15417-3tup610h`: HEAD `c8d889a87d0817c20cc8787408a2402a998ff03a`, reviewed code parent `158985d5da78c6a4b2055425d9dfb3c91aaaa8f1`, and Approved R1 `f3c4c2b341902bb70920c0163221c3cfafa80f1f` as an ancestor. The exact amendment 5 code candidate is `9c112bc4f7bef88021bf1695a27b514ff884167d`, parent `c8d889a87d0817c20cc8787408a2402a998ff03a`. It changes exactly this handoff and the existing executor test module; no branch switch, fetch, merge, push, live smoke, compatibility run, or running-operator mutation was performed.

The fold addresses the two terminal-review blocker SMELLs from retained artifact `/private/tmp/a2a-r2f1b-4h2r1.9IgXFc/r2-extension-final-review-output.md`, controller-rehashed SHA-256 `67f96e047f07145f66ccebbc584236c8f885578f9aed0eaed30c24c3edb54808`. That host path was not mounted into the Tier-3 writer, so the writer independently re-established the source finding from the typed task and checked-in handoff before mutation. Production cancellation behavior was not changed.

The first dispatch (`exec-e1e8be0981ca8942d263d6635eb32f20` / `attempt-6a0d3f8317451f973dbd9566235a419e`) refused frozen admission before provider prompt because the checked-in LSP MCP environment requires a provider-effect key; it is inadmissible as a writer turn. The sole provider writer was `exec-011d56b10018f6b7633640e5ab0b399a` / `attempt-33e89f3b77f112002689436f462e1666` through a validated one-agent/one-workflow Tier-3 config with no MCP delivery. Its output is `/private/tmp/a2a-r2f1b-4h2-am5.bZc9g0/amendment5-writer-output-v2.md`, SHA-256 `fce9e415349b34766db9dee3cb704f85d81b6a25d20f4da7a803b3bd81aba313`; cleanup completed and no managed container remained.

Code repair: the review-graph fail-fast frozen run-spec helper now takes explicit scheduler readiness. The active scheduler collection helper requests `Readiness::Armed`; `disarmed_external_root_cancel_marks_running_and_descendants_workflow_canceled` requests `Readiness::Disarmed`, asserts the frozen spec's `deadline_activation` is `ManualOnlyR2f1a` before executor startup, then runs the same executor path with `PostRecordingExitFaultForTest` argument `None` and the same three terminal primary-disposition assertions. Mechanical consolidation inside the existing test module removed custom assertion messages and an unused enum derive to keep the Rust line cap; assertion coverage and terminal cardinality checks remain.

Admissible RED-first evidence used a fresh direct `rustc --test` binary because root Cargo resolution was setup-blocked by unavailable registry/index entries (`a2a-lf`/`arc-swap`) and HTTP 403 from the CONNECT tunnel. Setup failures were not counted as RED.

Fresh local support rlib commands used before RED/GREEN workflow test compilation:

```bash
mkdir -p /tmp/a2a-r2f1b-am5
rustc --edition=2021 --crate-name bridge_core --crate-type lib crates/bridge-core/src/lib.rs -L dependency=target/debug/deps --extern async_stream=target/debug/deps/libasync_stream-12b053ee42b9776c.rlib --extern async_trait=target/debug/deps/libasync_trait-74f89434bf4b0db3.so --extern futures=target/debug/deps/libfutures-dc92b1102a6c9c0a.rlib --extern libc=target/debug/deps/liblibc-8844194d13f2ff95.rlib --extern ring=target/debug/deps/libring-9970c10e834c7569.rlib --extern serde=target/debug/deps/libserde-fcf88f2d060c8bf2.rlib --extern serde_json=target/debug/deps/libserde_json-b90567a03fd78732.rlib --extern thiserror=target/debug/deps/libthiserror-9d5dd55ca10cc186.rlib --extern tokio=target/debug/deps/libtokio-02ea95c19d9be400.rlib --extern tokio_stream=target/debug/deps/libtokio_stream-31c90f15f55c0c4a.rlib --extern toml=target/debug/deps/libtoml-f29d6f23cfecd801.rlib --extern tracing=target/debug/deps/libtracing-53a7bede155f7128.rlib -o /tmp/a2a-r2f1b-am5/libbridge_core.rlib
rustc --edition=2021 --crate-name bridge_observ --crate-type lib crates/bridge-observ/src/lib.rs -L dependency=target/debug/deps -L dependency=/tmp/a2a-r2f1b-am5 --extern bridge_core=/tmp/a2a-r2f1b-am5/libbridge_core.rlib --extern prometheus=target/debug/deps/libprometheus-56b24e61941b30ab.rlib --extern tokio=target/debug/deps/libtokio-02ea95c19d9be400.rlib --extern tracing=target/debug/deps/libtracing-53a7bede155f7128.rlib --extern tracing_subscriber=target/debug/deps/libtracing_subscriber-f3009a0b1e78cf24.rlib -o /tmp/a2a-r2f1b-am5/libbridge_observ.rlib
```

RED compile command:

```bash
rustc --edition=2021 --crate-name bridge_workflow --test crates/bridge-workflow/src/lib.rs -L dependency=target/debug/deps -L dependency=/tmp/a2a-r2f1b-am5 --extern async_stream=target/debug/deps/libasync_stream-12b053ee42b9776c.rlib --extern async_trait=target/debug/deps/libasync_trait-74f89434bf4b0db3.so --extern bridge_core=/tmp/a2a-r2f1b-am5/libbridge_core.rlib --extern bridge_observ=/tmp/a2a-r2f1b-am5/libbridge_observ.rlib --extern futures=target/debug/deps/libfutures-dc92b1102a6c9c0a.rlib --extern serde=target/debug/deps/libserde-fcf88f2d060c8bf2.rlib --extern serde_json=target/debug/deps/libserde_json-b90567a03fd78732.rlib --extern tokio=target/debug/deps/libtokio-02ea95c19d9be400.rlib --extern tokio_stream=target/debug/deps/libtokio_stream-31c90f15f55c0c4a.rlib --extern tokio_test=target/debug/deps/libtokio_test-b0bdd9b60ee03d2b.rlib --extern tokio_util=target/debug/deps/libtokio_util-87a0f89a64aa34aa.rlib --extern tracing=target/debug/deps/libtracing-53a7bede155f7128.rlib --extern trybuild=target/debug/deps/libtrybuild-45b3e1564a11452b.rlib -o /tmp/a2a-r2f1b-am5/red-bridge_workflow
```

RED run command:

```bash
/tmp/a2a-r2f1b-am5/red-bridge_workflow executor::tests::slice4h2_mux::disarmed_external_root_cancel_marks_running_and_descendants_workflow_canceled --exact --nocapture
```

RED total: 0 passed, 1 failed, 0 ignored, 0 measured, 160 filtered out. The failure was the required frozen-spec mismatch: left `AutomaticR2f1b`, right `ManualOnlyR2f1a`.

Focused GREEN compile/run commands:

```bash
rustc --edition=2021 --crate-name bridge_workflow --test crates/bridge-workflow/src/lib.rs -L dependency=target/debug/deps -L dependency=/tmp/a2a-r2f1b-am5 --extern async_stream=target/debug/deps/libasync_stream-12b053ee42b9776c.rlib --extern async_trait=target/debug/deps/libasync_trait-74f89434bf4b0db3.so --extern bridge_core=/tmp/a2a-r2f1b-am5/libbridge_core.rlib --extern bridge_observ=/tmp/a2a-r2f1b-am5/libbridge_observ.rlib --extern futures=target/debug/deps/libfutures-dc92b1102a6c9c0a.rlib --extern serde=target/debug/deps/libserde-fcf88f2d060c8bf2.rlib --extern serde_json=target/debug/deps/libserde_json-b90567a03fd78732.rlib --extern tokio=target/debug/deps/libtokio-02ea95c19d9be400.rlib --extern tokio_stream=target/debug/deps/libtokio_stream-31c90f15f55c0c4a.rlib --extern tokio_test=target/debug/deps/libtokio_test-b0bdd9b60ee03d2b.rlib --extern tokio_util=target/debug/deps/libtokio_util-87a0f89a64aa34aa.rlib --extern tracing=target/debug/deps/libtracing-53a7bede155f7128.rlib --extern trybuild=target/debug/deps/libtrybuild-45b3e1564a11452b.rlib -o /tmp/a2a-r2f1b-am5/green-bridge_workflow
/tmp/a2a-r2f1b-am5/green-bridge_workflow executor::tests::slice4h2_mux::disarmed_external_root_cancel_marks_running_and_descendants_workflow_canceled --exact --nocapture
```

Focused GREEN total: 1 passed, 0 failed, 0 ignored, 0 measured, 160 filtered out.

Complete slice population command:

```bash
/tmp/a2a-r2f1b-am5/green-bridge_workflow executor::tests::slice4h2_mux:: --nocapture
```

Complete slice total: 9 passed, 0 failed, 0 ignored, 0 measured, 152 filtered out.

Post-format cumulative added nonblank Rust lines in `crates/bridge-workflow/src/executor.rs` versus exact Approved R1 `f3c4c2b341902bb70920c0163221c3cfafa80f1f`: **316 / 320**. Independent controller verification in immutable toolchain image `sha256:bb09479fd020b22313782830d3a640e1b97be4a5a0ecd70e46fb5985f4ff3086` is green for `cargo fmt --all -- --check`, warnings-denied all-target/all-feature Clippy, locked build, candidate-built repository hygiene (**40 tracked artifacts / 8 validated example configs**), the exact corrected regression (**1 passed / 0 failed / 160 filtered**), and the complete 4H-2 module (**9 / 0 / 152**). The complete configured selection reported **99 suite summaries / 4,283 passed / 0 failed / 12 ignored / 0 measured / 716 filtered**. Its 21,372-byte log is `/private/tmp/a2a-r2f1b-4h2-am5.bZc9g0/amendment5-candidate-full-test.log`, SHA-256 `1e384aa3a92cc8584a5304212c13f20a21f3d80d2ccfd31a86d398c85cbb75c4`. Configured exclusions remain the entire `bridge-container` package and exactly three named host process tests.

The one authorized host-side `gpt-5.6-sol` / `xhigh` / hard-read-only cumulative re-review completed normally with joined cleanup: execution `exec-f4d43092eef32412e2e55cd4b2b34810`, attempt `attempt-3c3670b275a44eba47ea4037630ad4c8`. The exact review input is `/private/tmp/a2a-r2f1b-4h2-am5.bZc9g0/amendment5-rereview.md`, SHA-256 `509d8ee9d1b5fe2306ec4543a91ec09955419d11f6f6a38b2bd9784534ebc5f5`; the 19,288-byte output is `/private/tmp/a2a-r2f1b-4h2-am5.bZc9g0/amendment5-rereview-output.md`, SHA-256 `6deefbfa0215deef6cfe52e8a3b1c35fce72b1fc28e223b50a35d52f7ab76603`. The reviewer rebound clean branch/HEAD custody, read the cumulative `fd1f66f2..8bf84a12` diff, independently matched the 316 / 320 line count, and returned `VERDICT: REJECT`.

Both amendment 4 blocker `SMELL`s are closed: the named regression now binds the exact `Disarmed`/`ManualOnlyR2f1a` identity with behavioral RED evidence, and this handoff has one unambiguous current disposition. The re-review established one new current-production `WRONG / BLOCKER`: in both the passive and active paths, executor code samples the root cancel token to record external provenance and then samples it again during barrier acknowledgement. An external `cancel_task` arriving between the reads can make acknowledgement return `GlobalCancelAndDrain` while later writers durably project the root and running nodes as `CanceledPolicy` rather than `CanceledWorkflow`. The bounded proposed repair is to use one immutable cancellation sample for both observation and acknowledgement, with deterministic interleaving regressions for passive Disarmed and active acknowledgement paths plus a policy-only negative control.

Seven re-review `SMELL / DEFER` items remain recorded in the output: restore a discriminating production-select priority mutation test; retain cleanup-transfer guard ownership after projection; preserve custody on transfer error; deadline-bound preservation; distinguish live from historical cleanup owners; close protected `ApiBackend` transfer support; and retain or correctly locate the repository-local authority artifact. The first six overlap the already-parked R1/R3 evidence and design population; none reopens R3.

The authorized one-review cap is exhausted. This rejected cumulative candidate is parked: no further repair/review cycle, production redesign, restart, R3 work, merge, push, live smoke, compatibility execution, or running-operator mutation is authorized without new owner direction. Production readiness remains `Disarmed`.

---

## Amendment 4 terminal review rejection and definitive R2 park (historical, superseded by amendment 5)

The single owner-authorized Sol/xhigh final review bound exact clean candidate
`158985d5da78c6a4b2055425d9dfb3c91aaaa8f1`, exact amendment parent
`9b65978f846c34d352181227cba2b39ba45c9f6c`, and original slice base
`fd1f66f253c0f5128fed438f96b79dfabadc4d2f`. Review execution
`exec-dc19486f377b6dfe124d87f065166747`, attempt
`attempt-cf0703587347316a9711a09060f77e33`, completed with clean terminal and returned
`VERDICT: REJECT`. The 8,408-byte retained artifact is
`/private/tmp/a2a-r2f1b-4h2r1.9IgXFc/r2-extension-final-review-output.md`, SHA-256
`67f96e047f07145f66ccebbc584236c8f885578f9aed0eaed30c24c3edb54808`.

The review established no production `WRONG`, but found two `SMELL / BLOCKER` evidence and custody
failures. First, the required frozen-`Disarmed` regression calls `automatic_fail_fast_spec()`, which
freezes an `Armed`-derived run specification; current repository-wide readiness still makes the runtime
mux inactive, but the fixture does not satisfy the mandated production-reachable frozen contract and
would silently become active when global readiness changes. Second, this handoff simultaneously labeled
the new candidate and the earlier `be955468` rejection as current. The headings below are now explicitly
historical; this terminal section is now itself historical and superseded by amendment 5. This reconciliation records the
rejection and was not a repair or a rereview of the candidate.

The configured candidate full-suite gate remains incomplete rather than green. Two complete attempts each
reported 4,280 passed, 2 failed, 12 ignored, and 716 filtered, with different timing/process failures.
Exact parent `9b65978f` passed 4,280 / 0 in the same image/cache, and every failed candidate case passed by
exact-name rerun, so no constructible amendment regression was attributed. Format, warnings-denied Clippy,
locked build, hygiene, diff checks, the focused restored tests, and the 316 / 320 cumulative Rust-line cap
remain green, but none supersedes the missing full-suite pass or the review verdict.

This was amendment 4's single disclosed convergence extension. The review rejection parks R2
**definitively**: no repair, rereview, restart, R3 work, merge, push, live smoke, compatibility execution,
or running-operator mutation is authorized. Production readiness remains `Disarmed`. The containing
post-review handoff-only custody commit is not a replacement candidate and cannot reopen the verdict;
that historical controller instruction bound the reviewed code parent exactly to `158985d5`.

---

## Amendment 4H-2R2 external-root observation closure (historical reviewed candidate evidence)

Owner-authorized amendment 4 resumed only the retained clean clone on branch
`implement/impl-15417-3tup610h`. Pre-edit custody was verified exactly: HEAD
`9b65978f846c34d352181227cba2b39ba45c9f6c`, Rust-code parent
`be9554682f6ccee10759aa34bf5da151fa878f00`, and Approved R1 comparison base
`f3c4c2b341902bb70920c0163221c3cfafa80f1f` as an ancestor. The worktree was clean.
No branch switch, resume, merge, push, live smoke, compatibility run, network-authorized fetch,
operator mutation, checkpoint edit, or R3 work was performed. Production readiness remains
`Disarmed`.

### Closed boundary census and implementation

The pre-edit census was reinspected against `executor.rs` and closed as finite: one root token, one
`node_cancels` map, and the loop-local re-entry/projection boundaries are scheduler node-completion
wake, cleanup-preservation await, canceled-terminal projection after completion persistence/rich-event
flush, passive durability-barrier await before policy acknowledgement, yield re-entry before
stop/schedule, and final loop-exit/missing-node synthesis. No open-ended second root-cause mechanism was
found.

The repair adds one centralized loop-local `observe_external_root_cancellation!` transition. It is
first-write-wins: if the root token is canceled and no root origin exists, it records
`CanceledWorkflow` and seeds every still-running node with `CanceledWorkflow` via `entry(...).or_insert`.
`record_cancellation_primary!` remains the per-node first-cause writer. Both `GlobalCancelAndDrain`
sites still record policy root origin before self-signaling the root token, so policy-first behavior
continues to produce `CanceledPolicy` for affected running nodes and `NotStartedPolicy` downstream.

### RED and mutation evidence

Normal Cargo was setup-blocked in this retained environment: offline `cargo test -p bridge-workflow ...
--locked --offline` stopped before compilation because `arc-swap` is absent from the local crates.io
cache; a prior non-offline attempt was inadmissible because it attempted a crates.io index update and the
CONNECT tunnel returned HTTP 403. The no-network verification therefore used the retained direct
`rustc --test` path with existing `target/debug/deps` artifacts, outputting binaries under
`/tmp/a2a-r2f1b-am4/`.

With only the new tests present and incoming production cancellation logic temporarily restored, both exact
regressions compiled and failed genuinely:

- `executor::tests::slice4h2_mux::disarmed_external_root_cancel_marks_running_and_descendants_workflow_canceled`:
  0 passed, 1 failed, 0 ignored, 0 measured, 160 filtered out; terminal event cardinality was 2, required 3.
- `executor::tests::slice4h2_mux::external_root_cancel_during_empty_passive_barrier_marks_downstream_workflow_canceled`:
  0 passed, 1 failed, 0 ignored, 0 measured, 160 filtered out; `synth` primary was `NotStartedPolicy`,
  required `CanceledWorkflow`.

Two bounded temporary production mutations were compiled and run in the same direct-rustc environment, then
the fixed source was restored by checksum:

- Disabling non-barrier/root-wake observation made the Disarmed regression RED: 0 passed, 1 failed,
  0 ignored, 0 measured, 160 filtered out; terminal event cardinality was 2, required 3.
- Disabling passive-barrier pre-ack observation made the empty-map regression RED: 0 passed, 1 failed,
  0 ignored, 0 measured, 160 filtered out; `synth` was `NotStartedPolicy`, required `CanceledWorkflow`.

### Final GREEN and cap

Final no-network direct checks on restored source:

- `disarmed_external_root_cancel_marks_running_and_descendants_workflow_canceled`: 1 passed, 0 failed,
  0 ignored, 0 measured, 160 filtered out.
- `external_root_cancel_during_empty_passive_barrier_marks_downstream_workflow_canceled`: 1 passed,
  0 failed, 0 ignored, 0 measured, 160 filtered out.
- Complete `executor::tests::slice4h2_mux::`: 9 passed, 0 failed, 0 ignored, 0 measured,
  152 filtered out. This preserves workflow-first/policy-later, policy-first/workflow-later, policy-only,
  Approved-R1 wake lifecycle, and active parking regressions.
- Complete direct `bridge-workflow` library test binary: 161 passed, 0 failed, 0 ignored, 0 measured,
  0 filtered out.
- `cargo fmt --all -- --check`: GREEN.
- `git diff --check`: GREEN.

The cumulative added nonblank Rust line count in `crates/bridge-workflow/src/executor.rs` versus exact
Approved R1 `f3c4c2b341902bb70920c0163221c3cfafa80f1f` is **316 / 320**. Deletions were not credited.

Remaining R3 findings 4-8 are still reserved and unauthorized: cleanup-transfer guard/recovery custody,
transfer-failure custody, preservation deadline bounding, live-versus-historical cleanup-owner selection,
and protected `ApiBackend` transfer.

---

## Prior terminal R2 rejection and park (historical, superseded)

Final R2 review bound the exact clean candidate
`be9554682f6ccee10759aa34bf5da151fa878f00`, parent
`a35131034822d73f29897422ba87c0d9fb9e1bba`, and exact two-path patch SHA-256
`1f8ec09f65006b72c2deed546ad1467567bc155a2cf47e29ce4cbe2a48e6f2f9`. Review execution
`exec-5a62b9f6cf5d787fba62c579b85c900e`, attempt
`attempt-af3759f5fbab1e66a0bbccc062d5f069`, returned `VERDICT: REJECT`. The retained output is
`/private/tmp/a2a-r2f1b-4h2r1.9IgXFc/r2-final-review-output.md`, SHA-256
`22374f74ce4a415a6bc9bb0c234c69dbd2d2fb125ddd3e7f235be8d32e986e78`.

The terminal WRONG is constructible and production-reachable: root workflow/external provenance is latched
only through `record_cancellation_primary!`, so no latch occurs when ordinary cancellation settles nodes
without a scheduler writer or when an external cancellation arrives with an empty `node_cancels` map.
Those states can emit `CanceledNode`, fail missing-node terminalization with `InvalidStateTransition`, or let
a later policy global drain synthesize `NotStartedPolicy` even though external cancellation was observable
first. The bounded future repair is an unconditional first-write-wins root observation at scheduler wake and
after awaits, before canceled-terminal projection and missing-node synthesis; it needs real `Disarmed`
mid-run and empty-running-map barrier REDs.

This was the second rejection at the then-declared two-cycle R2 cap. That terminal park is historical and
superseded by the current amendment 5 fold-fix above. At the time, no third repair/review cycle, restart,
R3 work, merge, or push was authorized, and reviewed candidate `be955468` remained unmergeable. The
containing handoff-only custody commit was not a replacement candidate and did not alter or reopen the
review verdict. Production readiness remained `Disarmed`.

---

## Amendment 4 R2 final repair cycle (historical pre-final-review candidate)

### Exact custody and review disposition

This amendment was authored on retained branch `implement/impl-15417-3tup610h` with exact committed
parent HEAD `a35131034822d73f29897422ba87c0d9fb9e1bba`. Before this amendment, the working tree contained
only the authorized final-cycle repair in `crates/bridge-workflow/src/executor.rs`; this amendment makes
the complete authorized two-path set. The containing candidate commit cannot name its own SHA-1 inside
its content. The controller and final review must therefore bind the exact containing HEAD, confirm its
parent is `a35131034822d73f29897422ba87c0d9fb9e1bba`, and confirm that the parent-to-candidate path set is
exactly:

- `crates/bridge-workflow/src/executor.rs`
- `docs/superpowers/reviews/2026-08-24-r2f1b-slice4h2-handoff.md`

R2 review cycle 1 inspected committed candidate `a35131034822d73f29897422ba87c0d9fb9e1bba`
against original slice base `fd1f66f253c0f5128fed438f96b79dfabadc4d2f`. Review execution
`exec-0f15f563a721d84c180e24168bfa8a36`, attempt
`attempt-c8735e07643a765f5558652ce24fa4dc`, returned `VERDICT: REJECT`. The retained review artifact is
`/private/tmp/a2a-r2f1b-4h2r1.9IgXFc/r2-review-output.md`, SHA-256
`fe44c8fbefc70df7806d76a460e9aa79896a2a91a030c64689f1404c9bf2ae70`. Its three WRONG findings were:

1. external root cancellation observed while waiting could lose to a later policy provenance write;
2. policy-only `GlobalCancelAndDrain` could synthesize unscheduled nodes as `CanceledWorkflow` rather
   than `NotStartedPolicy`;
3. this handoff falsely described R1 custody as the current candidate.

The review's regression-path discrimination finding was classified SMELL and deferred, but this final
cycle adds path-preserving cases and mechanism-level mutation controls for both demonstrated WRONGs.
This is the second and final authorized R2 write/review cycle. A second rejection, or a repeated/open-class
finding, parks R2; R3 cannot begin without R2 approval.

### Provider attempts and local repair authority

Two identical bounded container-writer launches failed during ACP initialization before prompt
acceptance; their retained execution/attempt prefixes are `exec-6397` / `attempt-c3d` and
`exec-7b2d` / `attempt-d9b`. Both recorded
`prompt_may_have_been_accepted=false`. An instrumented third launch,
`exec-dfc8bd34d954ba698ec6df85b690a26b` / `attempt-e76a76fbe03795c38492a847409245c6`,
completed ACP initialization, session creation, and model configuration, then remained idle without a
child process, tool call, output artifact, or candidate change. The controller interrupted it and did not
send another provider prompt. None of these attempts is behavioral code evidence. The authorized final
cycle continued locally on the same retained artifact; there was no restart or replacement candidate.

### Final-cycle implementation

The scheduler now owns both per-node first cause and explicit root-cancellation origin. One local
`record_cancellation_primary!` operation records provenance before every child-token signal and preserves
the first write. If a policy action discovers an already-canceled root with no recorded root origin, it
first records external/workflow origin for the root and all currently running nodes. Policy
`GlobalCancelAndDrain` records `CanceledPolicy` as root origin before canceling the root. Missing-node
synthesis uses that explicit origin: external/workflow root cancellation yields `CanceledWorkflow`, while
policy-stopped admission yields `NotStartedPolicy`. Raw `cancel.is_cancelled()` remains a scheduling and
outcome signal, not the terminal-provenance source.

Every scheduler policy writer is routed through the recorder: ready-batch cutoff, durable-barrier sibling
cancellation and global drain, absolute-cutoff/mechanical-impossibility handling, fixed-grace expiry, and
the post-finalization policy action handler. R1 barrier multiplexing, exhaustion handling, retained
completion semantics, and active wait parking are unchanged. Production readiness remains `Disarmed`.

### RED, mutation discrimination, and current GREEN

Before production repair, two path-preserving tests compiled against unchanged `a3513103` production
source and failed in the verifier environment:

- `passive_external_cancellation_precedes_later_policy`: 0 passed, 1 failed, 158 filtered;
  observed `CanceledPolicy`, required `CanceledWorkflow`.
- `policy_global_drain_keeps_unscheduled_node_policy_stopped`: 0 passed, 1 failed, 158 filtered;
  observed `CanceledWorkflow`, required `NotStartedPolicy`.

Earlier attempts that stopped at linker `ENOSPC` selected no test and are inadmissible. Docker's global
filesystem later remained saturated despite bounded removal of rebuildable zero-container images and stale
Cargo cache executables, so final focused execution moved to the populated host target. Two same-host
temporary production mutations proved path discrimination and were restored immediately:

- disabling recognition of an already-canceled external root made the passive test RED with
  `CanceledPolicy` versus `CanceledWorkflow`;
- replacing explicit root-origin synthesis with raw `cancel.is_cancelled()` made the policy-only test RED
  with `CanceledWorkflow` versus `NotStartedPolicy`.

After each restoration, its exact test returned GREEN: 1 passed, 0 failed, 158 filtered. The cumulative
first-cause test also passed 1/0/158; the complete 4H-2 module passed 7/0/152; and the complete
`bridge-workflow` library passed 159, failed 0, ignored 0, filtered 0.

The configured hermetic-safe workspace command was then run on the host because the Docker filesystem
could not link another verifier binary. Its retained log,
`/private/tmp/a2a-r2f1b-4h2r1.9IgXFc/r2-final-host-full-test.log`, has SHA-256
`00f97a26610446a38eeb59ed4ace61dca25bd998794a4c11bfdd21c39eee8275` and reports 99 suite
summaries: 4,271 passed, 9 failed, 12 ignored, 0 measured, and 716 filtered. The only failed targets were
`fallback_plan_cli` (22 passed / 6 failed) and `smoke_cli` (11 passed / 3 failed); every failure stopped at
host `smoke.durable_evidence_unavailable` before the assertion under test.

The exact parent `a3513103` was exported to
`/private/tmp/a2a-r2-base-control-a351.1McyCx`; its executor SHA-256 matched `git show`. In the same host
toolchain and shared target, parent `fallback_plan_cli` produced the same 22/6 failures and parent
`smoke_cli` the same 11/3 failures, with the same persistence-first results. The nine failures are therefore
same-environment base failures, not attributed to this repair. `bridge-container` and the three named
host-process tests remain excluded exactly as configured; no ignored test was forced.

`cargo fmt --all -- --check`, warnings-denied all-target/all-feature Clippy, `cargo build --locked`,
`git diff --check`, and candidate-built `validate --repo-hygiene` are GREEN. Hygiene validated 40 tracked
artifacts and 8 example configs.

### Cap, exclusions, and remaining final-cycle gates

The independently counted cumulative R2 diff against exact Approved R1 commit
`f3c4c2b341902bb70920c0163221c3cfafa80f1f` adds **183 nonblank formatted Rust lines / 200 maximum**;
deletions are not credited. No manifest, lockfile, readiness gate, cleanup-transfer R3 path, release,
network fetch, live smoke, compatibility case, merge, or push changed or ran.

The configured verifier image remains unrunnable because Docker has no linker headroom; its host-equivalent
fmt/Clippy/build/test/hygiene gates and the exact same-host base controls above are the largest completed
subset. The remaining R2 gate is the final independent Sol/xhigh review of the containing commit. R3
findings 4-8 remain reserved and unauthorized until an R2 `APPROVE`.

---

## Amendment 3 R2 first-cause cancellation provenance (historical pre-review candidate)

### Custody, R1 approval, and scope

The owner-provided incoming custody was verified before edits: clean branch
`implement/impl-15417-3tup610h` at exact Approved R1 commit
`f3c4c2b341902bb70920c0163221c3cfafa80f1f`. HEAD remains that commit because this retained-clone
cycle must leave the patch staged for the controller rather than commit it locally.

This R2 cycle changes only cancellation provenance in
`crates/bridge-workflow/src/executor.rs` plus this handoff. Approved R1 barrier multiplexing,
exhaustion handling, completion retention, and active wait parking remain covered by the unchanged
slice4h2 tests. Production readiness remains `Disarmed`; the active seam remains test-only. R1's
forced-tie evidence smell and R3 findings 4-8 are unchanged and excluded.

### R2 implementation and complete cancellation-site inventory

The two accumulating `workflow_canceled` / `policy_canceled` sets and the workflow-dominant
`canceled_primary_disposition_v1` projection are removed. One
`BTreeMap<NodeId, NodePrimaryDispositionV1>` owns cancellation provenance. Every writer uses
`entry(...).or_insert(...)`, so the first observed scheduler cause is immutable.

The executor records the cause before signaling at every reachable scheduler cancellation site:

- cutoff ready-batch arbitration records `CanceledPolicy` before each child token;
- durable-barrier `CancelRunningSiblings` records policy before each child token;
- durable-barrier `GlobalCancelAndDrain` records policy for every in-flight node before the root token;
- workflow/external, absolute-cutoff, and mechanical-impossibility arms record the selected workflow
  or policy cause before each child token;
- fixed-grace expiry records policy before each child token;
- the later post-finalization action handler records policy before both targeted child cancellation and
  `GlobalCancelAndDrain` root cancellation.

Terminalization reads the owned record directly. A canceled node with no scheduler record is
source-backed: an observably canceled root token remains `CanceledWorkflow`; otherwise the terminal is
`CanceledNode`, never a guessed policy or workflow cause. The negative regression drives the former
through the real executor and separately proves every `codex`, `claude`, and synthesized `synth`
terminal appears exactly once.

### R2 RED-first evidence

Before any production edit, the formatted test-only regressions compiled against the incoming
production source with the retained offline artifacts, using the same direct `rustc ... lib.rs --test`
command documented in R1 and output
`/tmp/a2a-r2f1b-r2-red/bridge_workflow_tests`.

The final consolidated actual-scheduler RED command was:

```bash
/tmp/a2a-r2f1b-r2-red/bridge_workflow_tests --exact \
  executor::tests::slice4h2_mux::cancellation_primary_is_the_first_scheduler_cause \
  --nocapture --test-threads=1
```

Result: **0 passed / 1 failed / 0 ignored / 157 filtered**. The regression first completed the
workflow-then-policy and policy-only temporal rows, then failed the policy-then-workflow row after the
same in-flight `claude` node had observed both causes while its backend cancellation remained held open.
Its decoded canonical `NodeTerminalV1` evidence was exact:

```text
PolicyThenWorkflow
left: CanceledWorkflow
right: CanceledPolicy
```

The separate incoming-source negative command selected one real executor test and passed
**1 / 0 / 0 ignored / 157 filtered**, proving a preexisting root workflow cancellation did not become
policy cancellation or a missing-node `NotStartedPolicy` synthesis. Earlier per-row execution also
made the `PrimaryFailed` / `GlobalCancelAndDrain` policy-only gap explicit as
`CanceledWorkflow` versus expected `CanceledPolicy`; the final fixture uses committed fail-fast for the
policy-only `NotStartedPolicy` discriminator and reaches `GlobalCancelAndDrain` in the reverse-order row.

Excluded RED setup diagnostics: the first manual compile named serde with the wrong `.so` suffix; the
second retained two mistyped artifact hashes. Both stopped before compilation and are not test evidence.
The corrected retained-artifact compilation succeeded before the RED commands above.

### R2 GREEN and verification boundary

The final formatted source compiled warning-free to
`/tmp/a2a-r2f1b-r2-green/bridge_workflow_tests`. Final commands and exact results:

```bash
/tmp/a2a-r2f1b-r2-green/bridge_workflow_tests --exact \
  executor::tests::slice4h2_mux::cancellation_primary_is_the_first_scheduler_cause \
  --nocapture --test-threads=1
# 1 passed; 0 failed; 0 ignored; 156 filtered

/tmp/a2a-r2f1b-r2-green/bridge_workflow_tests \
  executor::tests::slice4h2_mux:: --nocapture --test-threads=1
# 5 passed; 0 failed; 0 ignored; 152 filtered

/tmp/a2a-r2f1b-r2-green/bridge_workflow_tests --test-threads=1
# 157 passed; 0 failed; 0 ignored; 0 filtered
```

The focused test drives all four structured rows: workflow then policy via the later
`GlobalCancelAndDrain`, targeted policy only with `NotStartedPolicy` synthesis, targeted policy then
workflow, and source-backed preexisting workflow cancellation. It decodes canonical terminal JSON for
all three graph nodes and requires exactly one structured `NodeFinished` event per node. The complete
focused module preserves all three Approved R1 regressions plus the active wait parking test.

The canonical no-network Cargo wrapper
`CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true cargo test -p bridge-workflow --lib --locked --
--test-threads=1` was attempted and excluded: dependency resolution stopped before compilation because
`arc-swap` is absent from the local cache. No online retry was permitted. The direct full library target
above is therefore the largest runnable `bridge-workflow` target in this retained environment. The
controller still owns the configured full verifier.

`cargo fmt --all -- --check` and `git diff --check` are GREEN. No live smoke, compatibility case,
provider turn, release, deployment, operator mutation, network command, ignored test, or R3 cleanup path
was exercised.

### R2 line cap, staged custody, and remaining gate

After formatting, an independent `git diff --unified=0 f3c4c2b3 -- executor.rs` addition-only count
reports **197 added nonblank Rust lines / 200 maximum**. Deletions were not credited.

Candidate custody remains the retained branch and unchanged HEAD `f3c4c2b3`, with exactly these two
controller-owned final paths to be staged:

- `crates/bridge-workflow/src/executor.rs`
- `docs/superpowers/reviews/2026-08-24-r2f1b-slice4h2-handoff.md`

The requested controller commit message is
`Preserve first-cause cancellation provenance (R2f1b 4H-2R2)`. The remaining gate is R3 findings 4-8:
cleanup transfer guard/recovery custody, transfer-failure custody, preservation deadline bounding,
live-versus-historical cleanup-owner selection, and protected `ApiBackend` transfer. None was changed.

---

## Approved amendment 3 R1 incoming custody (historical evidence)

The bounded correction for terminal findings 1-3 is locally GREEN on incoming HEAD
`37f72c68b6f399cd42fc3117bfbcc0ff0fabe8fd`. The pre-edit gate observed that exact HEAD and a clean
tree. Production readiness still comes from `scheduler_activation_readiness_v1()` and remains `Disarmed`;
the active scheduler seam used below is private to tests. The historical round-4 rejection is preserved
verbatim below and remains the disposition for findings 4-9.

### Current implementation

- The durable policy-trigger barrier is stored as one `BarrierFut` scheduler obligation and polled as
  arm 2. Creation, readiness, acknowledgement storage, evidence attachment, and controller action
  consumption are each single-shot. No active-scheduler path awaits the barrier inline.
- Empty `FuturesUnordered` is node-work exhaustion, not scheduler quiescence. The loop exits only after
  the barrier, acknowledgement, retained completion batch, and empty-node grace/cleanup obligations are
  discharged. The ordinary empty/Disarmed path still exits immediately.
- A completion removed by the select is retained across every higher-priority control handler. One
  post-handler guard resumes waiting only when neither a ready batch nor retained completion exists.
  Retained completions are sorted by `NodeId`, terminalized once, and emitted once with their retained
  durable evidence.
- The production changes and focused tests are confined to
  `crates/bridge-workflow/src/executor.rs`. This handoff is the only other changed path. No manifest,
  `Cargo.lock`, scheduler-arbitration source, readiness gate, or public admission surface changed.

### R1 RED-first evidence

Before the first production edit, the three focused regressions were compiled against exact incoming
production source with retained coherent dependency artifacts:

```bash
rustc --crate-name bridge_workflow --edition=2021 crates/bridge-workflow/src/lib.rs --test \
  -C debuginfo=2 -L dependency=target/debug/deps \
  --extern async_stream=target/debug/deps/libasync_stream-6547a16fee81fdd1.rlib \
  --extern tokio_util=target/debug/deps/libtokio_util-0a0019b0d5567da9.rlib \
  --extern futures=target/debug/deps/libfutures-c1aaa764c5e44a44.rlib \
  --extern bridge_core=target/debug/deps/libbridge_core-d09299c1b2067073.rlib \
  --extern tokio_test=target/debug/deps/libtokio_test-b0bdd9b60ee03d2b.rlib \
  --extern bridge_observ=target/debug/deps/libbridge_observ-7f2c379dd588f4cc.rlib \
  --extern trybuild=target/debug/deps/libtrybuild-45b3e1564a11452b.rlib \
  --extern serde_json=target/debug/deps/libserde_json-622dfe45f1596545.rlib \
  --extern tokio=target/debug/deps/libtokio-99656271c20ac753.rlib \
  --extern tokio_stream=target/debug/deps/libtokio_stream-1e1c84717c84d41c.rlib \
  --extern serde=target/debug/deps/libserde-0ac7c465fe4c3a12.rlib \
  --extern tracing=target/debug/deps/libtracing-1163a04c68910e9f.rlib \
  --extern async_trait=target/debug/deps/libasync_trait-791314a8fe4c931f.so \
  -o /tmp/a2a-r2f1b-r1-red/bridge_workflow_tests

/tmp/a2a-r2f1b-r1-red/bridge_workflow_tests --exact \
  executor::tests::slice4h2_mux::pending_barrier_keeps_cancellation_arm_live --nocapture
/tmp/a2a-r2f1b-r1-red/bridge_workflow_tests --exact \
  executor::tests::slice4h2_mux::exhausted_nodes_still_consume_global_cancel_acknowledgement --nocapture
/tmp/a2a-r2f1b-r1-red/bridge_workflow_tests --exact \
  executor::tests::slice4h2_mux::acknowledgement_winner_retains_simultaneous_completion_once --nocapture
```

All three were genuine behavioral RED with 0 passed, 1 failed, and 156 filtered out:

- Pending barrier: failed after 0.10 s with `a pending durable barrier blocked the armed cancellation
  input`; the barrier was deliberately unresolved and the armed cancellation input was not observed.
- Exhaustion: failed with `in-flight exhaustion bypassed GlobalCancelAndDrain`; the final trigger
  completion let the loop terminate before consuming its ready acknowledgement.
- Control winner: failed with `left: NotStartedPolicy`, `right: Failed`; the simultaneously consumed
  completion had been dropped instead of terminalized.

The final fixtures were mechanically consolidated for the size cap while preserving those same executor
state transitions and negative assertions. They still drive `run_with_diagnostic_context`, the production
select macro, controller finalization, durable barrier future, completion terminalization, and event stream.

### R1 GREEN and writer verification

The final formatted source compiled with the same `rustc` command, changing only the output to
`/tmp/a2a-r2f1b-r1-green/bridge_workflow_tests`. Commands and results:

```bash
/tmp/a2a-r2f1b-r1-green/bridge_workflow_tests \
  executor::tests::slice4h2_mux:: --nocapture --test-threads=1
# 5 passed; 0 failed; 152 filtered out

/tmp/a2a-r2f1b-r1-green/bridge_workflow_tests --test-threads=1
# 157 passed; 0 failed; 0 ignored; 0 filtered out

cargo fmt --all
cargo fmt --all -- --check
git diff --check
```

The five-test module includes all three new regressions plus the preserved active wait parking and
arm-order/provenance parity tests. Formatting and diff checks are GREEN. Added nonblank formatted Rust
lines against `37f72c68b6f399cd42fc3117bfbcc0ff0fabe8fd`: **347 / 350**.

Inadmissible probes, excluded from RED/GREEN evidence:

- An initial retained-artifact command misspelled the `tokio_util` hash (`...da9f.rlib`) and failed
  dependency resolution before compilation.
- Intermediate cap-consolidation builds caught unrelated test-fixture field/import spill and two invalid
  frozen-fixture shapes (`InvalidNodeIdentity` and `InvalidGraph`). These were compile/fixture diagnostics,
  not scheduler results; the exact incoming fields/imports were restored and the valid all-codex frozen
  review topology was used.
- No timeout, zero-selected test, dependency refusal, source-text assertion, helper-only mapping, or
  Disarmed parity result is counted as RED or GREEN. The normal Cargo dependency limitations remain as
  recorded in the historical section; the complete compiled library test population was the largest
  runnable writer check.

### Exclusions and remaining authority

Findings 4-8 remain reserved for R3: cleanup transfer guard/recovery-owner lifetime, transfer-failure
custody, preservation deadline bounding, current-owner selection, and protected `ApiBackend` transfer.
Finding 9 cancellation provenance remains reserved for R2. No cleanup-custody work, cancellation-provenance
work, roadmap/controller-document edit, release, live smoke, compatibility run, merge, push, or readiness
activation was performed. The operator still owns the exact configured full verifier and cumulative
Sol/xhigh review; this writer evidence does not arm production or supersede the historical rejection.

---


## Result

**REJECTED and parked at the disclosed four-round cap.** The final Sol/xhigh review inspected
candidate `3ce2e7c2db6b5e40107868d533e37baf7c1cfbcb` against original base
`fd1f66f253c0f5128fed438f96b79dfabadc4d2f` and returned nine `BLOCKER/WRONG` defects in the armed
scheduler path: an unmultiplexed barrier arm, exhaustion bypassing `GlobalCancelAndDrain`, completions
dropped when control arms continue, cleanup-transfer guard/recovery-owner loss, no guard on transfer
failure, an unbounded preserve await past the deadline, a stale append-only owner blocking live transfer,
`ApiBackend` inheriting the refusing default, and reverse-order cancellation misclassification.

Review execution `exec-ff0f27cfbe4c41c5662239b900be739d`, attempt
`attempt-ffef9128e7fcc3726751537fc2464734`, completed both reviewer and synthesizer and emitted
`VERDICT: REJECT`. No fifth repair/review round and no merge are authorized. The repair and verification
evidence below is retained, but it does not establish acceptance.

## Implemented wiring (not accepted)

The following records the intended implementation and its local evidence; the terminal review above
controls the disposition.

- The production select and its executable tests invoke the same `scheduler_select_v1!` definition.
  Its branches remain, in order: ready completions, durable barrier acknowledgement, workflow or
  external cancellation, fixed-grace expiry, absolute cutoff, mechanical impossibility, due warning,
  and the combined node/activity/control/clock wait.
- Every node future records its completion time from the shared attempt clock. A ready batch is
  passed to `arbitrate_scheduler_v1` with the current live readiness facts, absolute-cutoff timestamp,
  and in-flight node set. Every ready completion remains in the sorted batch; post-cutoff nodes and
  unfinished siblings are canceled before completion handling. A ready post-cutoff result is emitted
  once with a policy-canceled terminal rather than discarded.
- When `WorkflowOrExternalCancellation` wins, each then-in-flight node is added to
  `workflow_canceled` before its token is canceled. Terminal mapping returns `CanceledPolicy` only
  for policy membership without workflow membership; workflow membership therefore survives any
  later lower-priority policy membership. The regression also asserts the unchanged policy-only case.
- Cancellation handlers arm one cleanup deadline from the shared clock. When it is due, the executor
  reads every still-active owner from `WorkflowCleanupTracker`, preserves its checkout, and invokes
  `AgentBackend::transfer_cleanup_deadline_v1`; worktree, ACP-process, and container-reaper backends
  forward that call to the exact retained resource flight.
- A transferred or unknown cleanup settlement is retained per active node until completion handling.
  Its duration is merged into `NodeTerminalV1.cleanup`, with `UnknownLegacy` unless an existing
  cleanup failure is already worse.
- The module remains normally formatted; no `rustfmt::skip` was introduced.

## Executable scheduler evidence

- `workflow_cancel_winner_survives_later_policy_cancel` drives the production select macro through
  all eight priority rows, explicitly observes `WorkflowOrExternalCancellation`, then constructs a
  later `CancelRunningSiblings` policy fact. It asserts `CanceledWorkflow` for the combined state
  and `CanceledPolicy` for policy-only state.
- `active_armed_wait_path_polls_once_then_parks` invokes the production select macro with
  scheduler-active gating, no ready control or clock input, and a pending in-flight future. One manual
  poll returns `Pending` and the counter proves the in-flight future was polled exactly once. This is
  direct active/armed evidence, not Disarmed parity, source inspection, or delayed cancellation.
- The redundant local 4D ready-batch test was removed to meet the hard size ceiling. The dedicated
  `r2f1b_slice4d_scheduler_arbitration` integration tests continue to own cutoff classification,
  post-cutoff completion, cancellation-target, and node-order coverage.
- `fan_in_synth_receives_both_reviews_and_input` retains its bounded collect and exact representative
  start/finish/cleanup/terminal sequence assertion.
- `ownerless_proof_wiring_keeps_production_disarmed` retains the production refusal gate:
  readiness is `Disarmed` and policy remains `ManualOnlyR2f1a`.

## Round 4 RED and GREEN

The normal Cargo path could not resolve dependencies in this retained environment, so the focused
tests were compiled from the exact worktree source with the coherent retained dependency artifacts:

```bash
rustc --crate-name bridge_workflow --edition=2021 crates/bridge-workflow/src/lib.rs --test \
  -C debuginfo=2 -L dependency=target/debug/deps \
  --extern async_stream=target/debug/deps/libasync_stream-6547a16fee81fdd1.rlib \
  --extern tokio_util=target/debug/deps/libtokio_util-0a0019b0d5567da9.rlib \
  --extern futures=target/debug/deps/libfutures-c1aaa764c5e44a44.rlib \
  --extern bridge_core=target/debug/deps/libbridge_core-d09299c1b2067073.rlib \
  --extern tokio_test=target/debug/deps/libtokio_test-b0bdd9b60ee03d2b.rlib \
  --extern bridge_observ=target/debug/deps/libbridge_observ-7f2c379dd588f4cc.rlib \
  --extern trybuild=target/debug/deps/libtrybuild-45b3e1564a11452b.rlib \
  --extern serde_json=target/debug/deps/libserde_json-622dfe45f1596545.rlib \
  --extern tokio=target/debug/deps/libtokio-99656271c20ac753.rlib \
  --extern tokio_stream=target/debug/deps/libtokio_stream-1e1c84717c84d41c.rlib \
  --extern serde=target/debug/deps/libserde-0ac7c465fe4c3a12.rlib \
  --extern tracing=target/debug/deps/libtracing-1163a04c68910e9f.rlib \
  --extern async_trait=target/debug/deps/libasync_trait-791314a8fe4c931f.so \
  -o /tmp/a2a-r2f1b-4h2-red/bridge_workflow_tests
```

Genuine RED, run before the production provenance repair:

```bash
/tmp/a2a-r2f1b-4h2-red/bridge_workflow_tests \
  executor::tests::slice4h2_mux::workflow_cancel_winner_survives_later_policy_cancel \
  --exact --nocapture
```

Result: failed as required, with `left: CanceledPolicy`, `right: CanceledWorkflow`;
0 passed, 1 failed, 154 filtered out.

The final source was compiled with the same command, changing only the output to
`/tmp/a2a-r2f1b-4h2-red/bridge_workflow_green`. Focused GREEN commands:

```bash
/tmp/a2a-r2f1b-4h2-red/bridge_workflow_green \
  executor::tests::slice4h2_mux::workflow_cancel_winner_survives_later_policy_cancel \
  --exact --nocapture
/tmp/a2a-r2f1b-4h2-red/bridge_workflow_green \
  executor::tests::slice4h2_mux::active_armed_wait_path_polls_once_then_parks \
  --exact --nocapture
```

Each passed: 1 passed, 0 failed, 153 filtered out.

Excluded or inadmissible probes:

- `CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true cargo test -p bridge-workflow --lib
  executor::tests::slice4h2_mux::workflow_cancel_winner_survives_later_policy_cancel --locked --
  --exact` failed before compilation because `arc-swap` was absent from the local offline cache.
- The same Cargo command without offline mode failed before compilation because the configured
  crates.io CONNECT tunnel returned HTTP 403 while fetching `a2a-lf`.
- Docker is not installed in this environment, so no verifier-container substitute was admissible.
  None of these dependency or transport failures is counted as a code-test result.

## Frozen mutation control

- Path: `docs/superpowers/reviews/2026-08-24-r2f1b-slice4h2-mutation-control.patch`
- SHA-256: `ec09597aa456d861351a1a4128dce342c557162cafaa85416e0637861d81239c`
- Production mutation: swap the adjacent cancellation and durable-acknowledgement select arms.
- The retained candidate mutation run made the bridge-workflow library target the singleton added
  failure. Its priority assertion is now folded into
  `workflow_cancel_winner_survives_later_policy_cancel`.
- The mutation control was not rerun during round 4.

## Verification

- `cargo fmt --all`: green, and every formatted change was staged immediately.
- `cargo fmt --all -- --check`: green.
- `git diff --check`: green.
- Exact retained-artifact focused compile: green.
- Workflow-cancellation provenance regression: green.
- Active/armed wait-path parking regression: green.
- The exact configured verifier subsequently ran against this retained tree in fresh verifier
  containers: warnings-denied all-targets/all-features Clippy, locked debug build, and the configured
  workspace test command all exited 0.
- A pipefail-protected census of the same configured test population reported 99 suite summaries,
  4,270 passed, 0 failed, 12 ignored, 0 measured, and 722 filtered.
- The configured test environment excludes `bridge-container`, three named host process tests,
  `lock_release_failure_is_loud_not_silent`, and `staged_candidate_`.
- Repository hygiene, mutation control, release build, live smoke, and compatibility cases were not
  run in round 4. The single authorized round-4 repair provider turn is recorded above.

## Size, hashes, and invariants

- Pre-fold candidate commit: `27fb747f1a812b33a1fcbac61c68c4236ef248a8`.
- Added nonblank formatted Rust lines against
  `fd1f66f253c0f5128fed438f96b79dfabadc4d2f`: **700 / 700**.
- Base `executor.rs` SHA-256:
  `def9c4fc6dc174f7d744ef2554df4f428550a84725ee71129c7ff7127be684d4`.
- Final candidate `executor.rs` SHA-256:
  `d951e60f94fccf14d3cd75812c771ca3044f174561a06209314f82aa1e039781`.
- `Cargo.lock`, manifests, and `MAX_WORKTREE_CONFIGURES_IN_FLIGHT` are unchanged. Readiness remains
  `Disarmed`; production activation and readiness were not broadened.
