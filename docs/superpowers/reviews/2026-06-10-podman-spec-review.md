Both lenses returned full reviews; no node failed. The design shape is agreed sound by both — the disagreements are about severity, resolved inline below.

# Merged Spec Review — Podman Support (Slice 1)

## BLOCKER

**1. §3 — Runtime preflight scope is under-specified per command.**
The spec says to collect runtimes from "the snapshot's sandbox blocks and the verify config" and preflight "once at boot," but the commands have different natural scopes: `run-workflow` resolves one selected graph (`main.rs:1696-1703`) while the config may contain unused sandboxed agents plus API-only workflows (`examples/a2a-bridge.containerized.toml:342-356`) — a snapshot-wide preflight would make an API-only workflow require podman just because the config also defines containerized agents. `implement --resume` is a separate path (`main.rs:1493-1555`) the spec doesn't name.
*Resolution:* specify runtime collection per command — `run-workflow`: selected workflow nodes only; `implement` and `implement --resume`: edit/fix/review nodes plus `[verify]`; `serve`: eager all-agent preflight (see MAJOR 4 for reload).

**2. §1/§3 — `[verify].runtime` bypasses the allowlist the spec claims.**
The context section asserts "the runtime is also security-allowlisted," but `validate_sandbox` (`registry.rs:96-107`) only covers agent sandbox blocks; `VerifyToml.runtime` is parsed (`config.rs:378`) and passed straight to `compose_verify` (`verify.rs:147`) with no `allowed_cmds` check, and the default-union (`config.rs:738-753`) never includes it. Rigor rates this BLOCKER, Soundness rates it a context-accuracy note (pre-existing, and §3's preflight does probe the verify runtime's *existence*); Rigor is right for a spec review — the spec asserts a security invariant the design leans on, and that claim must be made true or explicitly weakened before planning, though Soundness's facts mean the fix is small.
*Resolution:* decide and state whether `[verify].runtime` is allowlist-gated. If yes, add the validation + tests to the slice; if no, correct the context sentence and justify the exemption.

## MAJOR

**3. §6 — The smoke never exercises the `:rw` implementor or verify, the two highest-variance surfaces.**
Step 2 makes the toolchain image conditional, step 5 runs only `:ro` readers, and nothing writes. §5's uid row credits rootless podman with "native userns remap," but that describes Linux-native rootless; the actual target (macOS → `podman machine` → virtiofs) is the VM-remap category, and the B2b-1-flagged round-trip (container writes the clone's `.git`/index → host runs `git commit` → `safe.directory`/ownership) is untested. Podman could ship "supported" and then fail on first `implement` on the work Mac — the machine this slice exists for.
*Resolution:* add an unconditional `:rw` step (minimal `implement`, or at least container-write + host `git status`/commit on the bind mount) and one gating verify command, converting §5's "none (handled by runtime)" cell from assertion to evidence.

**4. §3 — Preflight lifecycle ignores serve hot-reload and runtime agent-adds.**
`serve` hot-reloads snapshots (`main.rs:2743-2760`) and registry `apply` (`registry.rs:353`) admits new sandboxes at runtime; a boot-only preflight lets a later edit introduce `runtime = "podman"` and fail deep in the first spawn — exactly the cryptic failure §3 exists to remove.
*Resolution:* preflight watched snapshots before apply (keep last-good on failure), or wire the pure helper into the apply path; at minimum, document the gap as a decision.

**5. §3/§6 — The `containers` CLI is omitted from the preflight and error story.**
The spec says the runtime is threaded to the `containers` CLI and §6 step 6 expects it to list/reap under podman, but §3's preflight names only `serve`/`run-workflow`/`implement`, and current `containers list` silently swallows runtime command failures (`main.rs:2449-2461`).
*Resolution:* state whether `containers` preflights, and require it to fail loudly when the runtime is unavailable.

**6. §2 — The egress script's semantics are incomplete and version-fragile.**
"Idempotent" is defined only for network creation: `podman run -d --name a2a-egress-proxy` hard-fails on the second run, yet re-running `-up.sh` is the natural recovery path — especially since `--restart unless-stopped` under daemonless podman doesn't survive `podman machine stop/start` (proxies can be silently absent after a machine restart). The commands are cwd-dependent, and the relative `-v ./tinyproxy.verify.filter:…` source is a compose feature — raw `docker run` rejects it and podman support is version-bound, so the absolute-path fix is necessary, not hygiene. Teardown, rebuilt images, and changed filters are unaddressed.
*Resolution:* require the scripts to self-locate (`cd "$(dirname "$0")"`), use absolute bind paths, `rm -f` named proxies before `run` (the reaper's own idiom), tolerate absent resources on `-down.sh` (proxies before networks), and document "re-run `-up.sh` after `podman machine start`" as the supported recovery.

**7. §2/§4 — Image build ownership is ambiguous.**
"The script (or docs) covers `podman build`" leaves the supported operator path for `a2a-agent-reader`/`a2a-toolchain` undefined.
*Resolution:* pick one — script manages only networks/proxies and docs give exact build commands, or the script grows explicit image-build steps.

**8. §1/§8 — The duplicated podman example config has no test and no parity pin, and its exact contents are ambiguous.**
Repo convention pins shipped examples with parse tests (`main.rs:3548`); §8 adds nothing for the podman copy, so every future docker-example edit silently drifts the one artifact docs and smoke point at. The spec also leaves the final `allowed_cmds` value (`["podman"]` vs `["docker","podman"]`) and the fate of docker-specific header comments (`examples/a2a-bridge.containerized.toml:3-4`) unspecified.
*Resolution:* at minimum a parse/validate test plus a structural-parity assertion (same blocks, diffs confined to `runtime`/`allowed_cmds`) and exact final values stated in §1. The alternative `default_runtime` knob is the durable fix but contradicts §3's "only Rust change" sentence and touches the default-union (`config.rs:745`) — decide it explicitly in-or-out of scope.

**9. §6 — The egress-lock smoke step is not a verifiable acceptance test.**
"Reaches the provider only via the proxy" names no commands or expected results.
*Resolution:* enumerate: proxied allowed host succeeds; direct unproxied request on the internal net fails; proxied disallowed host fails; proxy log shows the refusal; verify-proxy allows crates/GitHub while denying provider hosts.

## MINOR

**10. §2/§5 — DNS on `--internal` networks is the unaudited parity risk.**
Every locked workload reaches its proxy by container name (`http://a2a-egress-proxy:8888`, `http://a2a-verify-proxy:8888`), and aardvark-dns historically didn't serve internal networks (fixed ~podman 4.5/netavark 1.6 — version boundary from memory, unverified).
*Resolution:* state a minimum podman version in docs/preflight hint and make name-resolution-over-the-internal-net an explicit assert in smoke step 5.

**11. §7 — The multi-network risk entry is likely stale.**
Modern podman (≥4.0) accepts repeated `--network` at `run`; the `run` + `network connect` two-step is unnecessary and leaves a brief one-net window.
*Resolution:* correct the entry, or keep the two-step deliberately for old-podman portability and say so.

**12. §3 — Preflight details lack testable criteria.**
"Short timeout" is unfalsifiable; stderr handling and exact failure wording are unstated.
*Resolution:* pin the timeout value, stderr disposition, error text shape, and that `<runtime> version` runs with no extra env/cwd assumptions.

**Verdict:** not ready to plan — resolve the two blockers (per-command preflight scope; the verify-runtime allowlist decision) and the named §2/§6 specifications (script idempotency/paths, `:rw`+verify smoke coverage, concrete egress assertions, example-config pinning) first; the architecture itself is sound and needs no reshaping.