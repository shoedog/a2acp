# serve lifecycle & operator ergonomics (post-R2g UX and supervision)

**Roadmap:** H1-4 (★★) · **Labels:** `kind:enhancement`, `area:serve`, `area:cli`, `area:ops`, `priority:p2`, `status:triage`
**Origin:** `SSOT_AGENTS_BRIDGE_COORDINATION.md` (live cross-repo request).
**Dependency:** reliability R2g owns stable ingress, identity/readiness/affinity, safe store ownership, drain, and
side-by-side promotion/rollback. This stub must consume that contract and must not duplicate it.

## Problem
`serve` is a bare foreground process: no daemonization, no PID/owner record, no auto-port selection, no
`/health` endpoint, and no way for a client to verify *which config* a bound server is running (HTTP 200 on
the Agent Card cannot distinguish two differently configured servers). The operator compensates with
hand-written `SERVICE.md` conventions, an ownership-ledger-by-context-id, and a creds-refresh launchd plist
that hardcodes a checkout path. This is friction today, not hypothetically.

## Scope (post-R2g operator ergonomics)
- [ ] Present R2g's release/process identity, readiness, affinity, drain, and recovery evidence through a friendly
      local operator CLI; do not create a second fingerprint/readiness protocol.
- [ ] Build discovery and diagnostics around the merged execution/attempt/task/session identities rather than a
      hand-maintained context-only ownership ledger.
- [ ] Fix the creds-refresh plist hardcoded path (`/Users/wesleyjinks/code/a2a-bridge/...`) — portability bug.
- [ ] **Later / separate decision:** a supervised launchd/systemd contract with a PID/owner record. Do not
      introduce daemon lifecycle machinery before R2g defines promotion, rollback, and predecessor drain.

## Non-goals / guardrails
- A client must never infer ownership from an occupied port, a tmux name, or a stale PID, and must never
  opportunistically start/replace/kill `serve`. Encode an explicit server-owner contract instead.

## Value
Removes real, current cross-repo friction and turns R2g's identity groundwork into usable operator experience for
team mode (H3-3), budgets (H1-2), and federation (H3-2) without prematurely committing to daemon machinery.
