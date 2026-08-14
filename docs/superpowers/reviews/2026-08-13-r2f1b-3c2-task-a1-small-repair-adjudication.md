# R2f1b 3c2 Task A1 small-repair adjudication

Date: 2026-08-13

> **Superseded 2026-08-14.** The owner separately authorized one bounded
> continuation on this preserved artifact. Exact commit `5cbeea1e` fixes both
> WRONGs recorded below and its sole Sol/xhigh closure returned `APPROVE` with
> no WRONG or SMELL findings. This file remains the historical adjudication of
> `6616753b`; current status is owned by the
> [owner-extension adjudication](2026-08-14-r2f1b-3c2-task-a1-owner-extension-adjudication.md)
> and [closure](2026-08-14-r2f1b-3c2-task-a1-owner-extension-sol-closure.md).

Landed base: `42249b3d926b49afd9d0dbd213d0ee3d3e459af6`

Rejected input: `bc262ad466b45470cd44fceda8224a36b2ba77b2`

Small-repair candidate: `6616753bf479d8775381eb9ef1d7237f5660514c`

Retained clone:
`/Users/wesleyjinks/code/.a2a-implement/impl-77617-f18mbkc5`

## Verdict

**THE SMALL A1 REPAIR FIXES ALL THREE INHERITED WRONGS BUT REMAINS REJECTED.
A1 IS PARKED AT THE THREAT-MODEL/DESIGN BOUNDARY. DO NOT INTEGRATE IT OR
DISPATCH A2.**

The one allowed implementation turn plus a narrow operator correction stayed
inside the declared +100 production/+250 total cap. The one allowed closure
review then found two new constructible instances of the same open
uncooperative-namespace-peer class. Per convergence discipline, another repair
or review would be a silent cap extension.

This is not evidence that the candidate should be scrapped. It remains useful
salvage for required identity, portable child names/intents, no-replace capture,
typed unsupported outcomes, and the three inherited regressions. It is also not
an integrable artifact under the existing arbitrary-peer custody contract.

## Exact execution and gate record

The bounded repair task fixed only:

1. refusal before mutation when complete required identity is unavailable;
2. no custody inspection/adoption/restoration after a failed capture rename;
3. portable child-name/intent construction and a callable non-Unix
   `CompileUnsupported` stub; and
4. the directly related parser/boundary regressions.

The exact committed diff changes one file, with 97 changed production lines and
235 total changed lines. Host verification at `6616753b`:

- `custody_v2`: 9 passed / 0 failed;
- `fs_custody`: 75 passed / 0 failed;
- workspace all-feature suite: **3,997 passed / 0 failed / 13 ignored**;
- all-target/all-feature check and warnings-denied Clippy: exit 0;
- locked release `a2a-bridge` build: exit 0;
- `cargo deny check`: advisories, bans, licenses, and sources okay;
- repository hygiene: 40 tracked artifacts / 8 example configs;
- formatter and diff checks: exit 0.

Actual non-Unix compilation/execution remains excluded. The known Mac-to-MSVC
route fails first in `ring` for missing MSVC headers, before `bridge-core`, so it
cannot green or red the A1 stub.

The repair workflow was `exec-a62be255b31d85fd965d95901c3fbecb` /
`attempt-8d16357272da7607e2d378deac15bea1`. The closure was
`exec-0ee009bac4882d0bcc4da06badd4777f` /
`attempt-b1a26506540f87cea5dc44410034fa45`; its durable mirror is the
[Sol/xhigh closure](2026-08-13-r2f1b-3c2-task-a1-small-repair-sol-closure.md).

## Closure classification

The reviewer marked every inherited WRONG **FIXED**. Its independent pass then
constructed:

1. successful capture followed by custody substitution before restoration,
   moving unrelated C into target; and
2. error-after-effect capture followed by a hard-link of the same object back
   to target, falsely proving `RefusedNoEffect` while custody remains occupied.

Both require a writable namespace peer in a very small rename window. That
mechanism is central under the old arbitrary-peer filesystem custody threat
model, but it is not automatically justified for a single-operator application
that can enforce one live process, owner-private state, and quiet-period
updates. The next action is therefore an owner threat-model/program-scope
decision, not a fifth custody repair.

No A2, B-G, fold, push, CI, production V3 activation, provider action,
deployment, or running-operator mutation follows from this record. The
two-field cleanup carry-forward remains binding in whichever later slice first
arms production V3 or wraps `ContainerRw`.
