# G-C2b3b verification — 2026-10-27

Base: `8a431cc04cb957d00e5c816dc85a6fa40d1528e9` (#242).
Lane: Linux aarch64 container, Rust 1.94.0, `/opt/git/bin/git` 2.54.0; fake providers only.
All Cargo verification used `CARGO_INCREMENTAL=0`; nextest used
`A2A_LANE_GIT=/opt/git/bin/git PATH=/opt/git/bin:$PATH`. No additional test filters or exclusions
were applied to the final workspace run. This is container evidence, not host/provider compatibility evidence.

The admitted unary driver is shared with eligible SSE turns. Successful events enter the outgoing
channel immediately; publication latches exposure independently of provider replay evidence or channel
occupancy. Successor session keys remain internal; SSE context IDs remain the caller's task ID.
The accounting projection allowlist follows the extracted response boundary.

Same-environment base-red: SSE variants of `served_safe_rejection_routes`,
`served_chain_is_frozen_and_finite`, cleanup cancel/disconnect/release controls, and
`production_billing_per_visited_candidate` fail behaviorally on the base: no successor runs.
Initial focused served/direct/production-billing run: **47 passed**. The independent exposure
proof also passes, using a real translator prompt-open rejection, checked cleanup, and a consumed
status-only channel event; its otherwise identical unexposed control transitions successfully.

Disposable mutations, restored before final verification, each produced a behavioral assertion failure:

- Remove only `ServedEventOutput`'s exposure check: independent exposure proof fails.
- Replace sticky exposure with outgoing-channel occupancy: the same proof fails after its receiver drains.
- Empty only the SSE driver's frozen chain: `served_safe_rejection_routes` fails to receive successor success.

| Final gate | Result |
| --- | --- |
| `cargo nextest run --workspace --no-fail-fast` | 5,984 passed, 3 failed, 13 ignored; 5,987 tests across 113 binaries |
| `cargo test --workspace --doc` | 11 passed, 0 failed, 0 ignored; 16 packages |
| `cargo check --workspace --all-targets` | Passed |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| `cargo fmt --all -- --check` | Passed (toolchain bin directory on PATH) |
| `cargo run -p a2a-bridge -- validate --repo-hygiene` | Passed; 9 example configs validated |
| `accounting_production::uncapped_order_and_wire_control` | Passed on base and candidate; 6,574 bytes byte-identical |

Revision 2 refuses invalid frozen source admission, checks existing state before fresh SSE admission,
and retains task-addressable cancellation ownership through resolution, with a final pre-configuration check.
All three new regressions pass in the full run and fail against rejected commit `151d8b9f`.
The exposure proof now uses the actual published latch and a fresh unexposed control; deleting the latch fails it.

Full-run failures; isolated candidate and base-source controls each pass **3/3**:
- `smoke::tests::run_attempt_prompt_barrier_uses_real_registry_cleanup_and_finalization` (deadline-dependent phase).
- `fallback_billing_fixture::fallback_target_terminal_evidence_is_resolved_before_aggregate_projection` (10 s timeout).
- `bridge-api::backend::tests::settlement_refusal_does_not_mask_the_provider_failure` (request-count timeout).
Base-source controls rebuild server and direct-fixture sources from `8a431cc0`, then run the same three tests serially.
Separate four-process lifecycle controls on rejected HEAD also time out (4 passed, 3 failed); no expectations or timeouts changed.

Wire receipt SHA-256: `8db30cf3512d21f51b4d811e560fda92d1475519b6abf47dbdd44d71564a9e84`.
No receipt baseline was updated. Unchanged unary/shared policy, provider-poll, MCP, and cold-binding
witnesses are reused through the focused and full workspace runs.

Exact existing ignored tests (no live turns were run):

- `a2a-bridge::e2e_acp_codex::real_codex_acp_prompt_round_trip_with_mode_applied`
- `a2a-bridge::e2e_acp_codex::unauthenticated_codex_acp_surfaces_agent_not_authenticated`
- `a2a-bridge::e2e_acp_kiro::real_kiro_acp_prompt_round_trip_yields_pong_then_done`
- `a2a-bridge::e2e_delegate_bridge::bridge_a_delegates_through_bridge_b_to_kiro`
- `a2a-bridge::e2e_fanout_bridge::bridge_a_fanout_through_bridge_b_to_kiro`
- `a2a-bridge::e2e_kiro::real_kiro_round_trip_returns_pong`
- `a2a-bridge::e2e_registry::claude_warm_two_turns_via_acp`
- `a2a-bridge::e2e_registry::live_edit_changes_new_session_model`
- `a2a-bridge::e2e_registry::override_applies`
- `a2a-bridge::e2e_registry::route_to_each_agent_by_id`
- `a2a-bridge::e2e_registry::route_to_each_of_three_agents_by_id`
- `bridge-api::live_ollama::api_live_two_turns`
- `bridge-container::tests::docker_identity_template_round_trips_all_a2a_labels`
