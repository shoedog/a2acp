# Fable R1 compatibility disposition

- **Date:** 2026-07-11
- **Bridge source:** `c8afc9d1f9ffe94bbd67463936b560f9c0a55190`
- **Fresh executable:** `/private/tmp/a2a-bridge-r1-target/debug/a2a-bridge`
- **Host:** Darwin 25.5.0 arm64
- **Reader image:** `sha256:f80543261786e5d4d818f6151e1e4b033383840d0b14e07c530109ef61d6a3ef` (Linux arm64)
- **Prompt:** `Reply exactly PONG. Do not use tools.`
- **Fable policy:** `A2A_BRIDGE_ALLOW_FABLE=1`; raw model `claude-fable-5[1m]`; effort `xhigh`

## Verdict

Fable is supported on the tested host path with both `claude-agent-acp` 0.44.0 and 0.55.0. It is
also supported in the tested reader image with 0.55.0 when the isolated Claude home mounts both the
credential copy and the pinned minimal settings file in
`deploy/containers/claude-fable-settings.json`.

The historical host failure was not adapter-version drift and was not Fable-specific. The ACP/bridge
process was launched inside a managed execution sandbox with DNS disabled. Claude's debug log recorded
`getaddrinfo ENOTFOUND api.anthropic.com`; the same ACP sequence completed immediately when launched
outside that sandbox. Re-authentication is not a remedy for this failure.

## Exact components

| Lane | Adapter | Agent SDK | Bundled Claude | Node | Auth |
|---|---:|---:|---:|---:|---|
| Host installed | 0.44.0 | 0.3.170 | 2.1.170 | 26.0.0 | Ambient host subscription credential |
| Host isolated candidate | 0.55.0 | 0.3.198 | 2.1.198 | 26.0.0 | Ambient host subscription credential |
| Reader image | 0.55.0 | 0.3.198 | 2.1.198 | 24.16.0 | `pre_authenticated=true`; isolated credential file |

The user's separately installed direct Claude CLI was 2.1.207. Both the 2.1.170 and 2.1.198 bundled
binaries also completed direct Fable prompts, so a bundled-binary regression was ruled out.

## Matched results

| Environment | Adapter | Model / effort | Direct ACP | Bridge | Result |
|---|---:|---|---:|---:|---|
| Host execution | 0.44.0 | Fable / xhigh | PASS (about 3.6 s) | PASS (about 6.7 s) | `PONG` |
| Host execution | 0.44.0 | Sonnet / high | PASS (about 2.2 s) | PASS (about 2.7 s) | `PONG` |
| Host execution | 0.55.0 | Fable / xhigh | PASS (about 4.0 s) | PASS (about 7.7 s) | `PONG` |
| Host execution | 0.55.0 | Sonnet / high | PASS (about 2.8 s) | PASS (about 2.8 s) | `PONG` |
| Managed no-egress sandbox | 0.44.0 | Fable and Sonnet | FAIL | historical FAIL | DNS unavailable; prompt retried/hung |
| Managed no-egress sandbox | 0.55.0 | Fable and Sonnet | FAIL | not separately run | Direct ACP hit the same DNS failure; the bridge control was not repeated there |
| Reader image, credential only | 0.55.0 | Fable / xhigh | not billed | FAIL before prompt | Fable not advertised |
| Reader image, credential + pinned settings | 0.55.0 | Fable / xhigh | n/a | PASS (about 5.1 s artifact-exact repeat; earlier cold run about 198 s) | `PONG` |

The 0.44.0 Sonnet surface rejected `xhigh` after the model switch but accepted `high`; the clean
non-Fable control therefore uses the common advertised `high` level. That is real capability-surface
drift, but it did not cause the Fable incident.

## Hypothesis, probe, result log

1. **Adapter-version mismatch.** Prediction: 0.44.0 fails while 0.55.0 passes. Falsifier: both versions
   behave the same under a matched environment. Result: falsified; both pass on the host and both direct
   ACP controls fail in the no-egress sandbox. The 0.55.0 bridge control was not separately repeated there.
2. **Host Claude config tree exhausts file watchers.** Prediction: an isolated `CLAUDE_CONFIG_DIR` or a
   larger file-descriptor limit removes the failure. Result: falsified. An `EMFILE` settings-watcher
   warning persisted for an empty directory and with a 65,536 soft limit; a trivial Node watcher still
   worked. The warning was correlated noise, not the prompt cause.
3. **Bundled Claude binary regression.** Prediction: the SDK-bundled binary fails when invoked directly.
   Result: falsified; bundled 2.1.198 returned Fable `PONG` in about 5.7 seconds.
4. **ACP or SDK protocol framing.** Prediction: a direct SDK `query()` reproduces a protocol error.
   Result: it reproduced exponential `api_retry` events, but the CLI debug file revealed the deeper
   cause: `ENOTFOUND api.anthropic.com`.
5. **Execution-environment egress.** Prediction: the exact ACP harness passes outside the managed
   sandbox. Result: confirmed; 0.55.0 Fable returned `PONG` in about 4 seconds, and the full bridge
   returned it with the explicit Fable gate.
6. **Reader adapter cannot use Fable.** Prediction: 0.55.0 still fails after credentials and egress are
   healthy. Result: refined. Credential-only isolation omitted Fable from `session/new`; mounting only
   the pinned model/effort settings made it advertised and the real reader turn passed.

## Changes justified by this evidence

- `doctor` fails a Fable-configured agent when `A2A_BRIDGE_ALLOW_FABLE` is not `1/true`.
- `doctor` warns when containerized `claude-agent-acp` pins Fable without a settings-file mount.
- The reader settings template contains only model and effort. Do not mount the full host
  `~/.claude/settings.json` or `~/.claude.json`; they contain unrelated policy, hooks, project state,
  and account caches.
- The managed environment's network marker is inherited by approved host commands, so it is not a
  reliable doctor signal. The separating evidence is the actual DNS/ACP control in each execution mode.

## Still unverified

- No representative full review was rerun in the reader image; the bounded minimal turn is the R1 gate.
- This PASS applies to the exact image digest above, not a future rebuild from the same Containerfile.
- Long-term latency and repeated-turn stability were not measured; the two reader turns varied from
  about 5.1 seconds to about 198 seconds.
- The bridge still collapses some prompt/transport failures into `AgentCrashed`; phase/error retention is
  R2, not closed by this disposition.
