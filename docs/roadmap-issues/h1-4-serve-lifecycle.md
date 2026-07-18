# serve lifecycle & operator ergonomics (identity + preflight before daemon machinery)

**Roadmap:** H1-4 (★★) · **Labels:** `kind:enhancement`, `area:serve`, `area:cli`, `area:ops`, `priority:p2`, `status:triage`
**Origin:** `SSOT_AGENTS_BRIDGE_COORDINATION.md` (live cross-repo request).

## Problem
`serve` is a bare foreground process: no daemonization, no PID/owner record, no auto-port selection, no
`/health` endpoint, and no way for a client to verify *which config* a bound server is running (HTTP 200 on
the Agent Card cannot distinguish two differently configured servers). The operator compensates with
hand-written `SERVICE.md` conventions, an ownership-ledger-by-context-id, and a creds-refresh launchd plist
that hardcodes a checkout path. This is friction today, not hypothetically.

## Scope (sequence deliberately — identity/preflight first, daemon machinery only after)
- [ ] Config-fingerprint on the Agent Card so a client can confirm it's talking to the intended server.
- [ ] `submit`/client preflight that checks the fingerprint + bound config before sending.
- [ ] A real readiness signal: a `/health` endpoint, or formally bless `GET /.well-known/agent-card.json` as
      the readiness contract.
- [ ] Generalize the hand-run ownership ledger (context-id acquire/release; a coordinated rebuild waits for an
      empty ledger) into a first-class, queryable serve concept. Design the identity model as the seed of
      team/multi-user mode (H3-3).
- [ ] Fix the creds-refresh plist hardcoded path (`/Users/wesleyjinks/code/a2a-bridge/...`) — portability bug.
- [ ] **Later / separate decision:** a supervised launchd/systemd contract with a PID/owner record. Do not
      introduce daemon lifecycle machinery in this issue.

## Non-goals / guardrails
- A client must never infer ownership from an occupied port, a tmux name, or a stale PID, and must never
  opportunistically start/replace/kill `serve`. Encode an explicit server-owner contract instead.

## Value
Removes real, current cross-repo friction and lays the identity groundwork for team mode (H3-3), budgets
(H1-2), and federation (H3-2) without prematurely committing to daemon machinery.
