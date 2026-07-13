# Post-work bridge wedge incidents — 2026-07-12

- **Status:** operator evidence; root cause unknown
- **Repository under work:** `/Users/wesleyjinks/code/stockTrading`
- **Bridge build:** `target/release/a2a-bridge` from `feat/m4-slice3a-ownership-finalization` at `baf4d63`
- **Follow-up owner:** [R2f phase-aware liveness and safe takeover](plans/2026-07-11-r2f-phase-aware-liveness.md)

Four `run-workflow` invocations completed useful file work and then remained at zero observed CPU without
writing `--out` or exiting. The operator sent `SIGTERM` only to each affected bridge run and shell wrapper.
High- and xhigh-effort runs from the same work session exited normally and are negative controls. The runs
used `codex-acp` with `auth_method="none"`, Prism MCP, `danger-full-access`, and
`approval_policy=never` on the operator's macOS development machine.

## Incident A — Luna/max implement-easy

- rollout: `019f589d-566f-74a2-bcea-d21a46455882`, started 17:15:34;
- operator task: `bl9juj9vu`;
- config/workflow: `stockTrading-openai-impl.toml` / `implement-easy`;
- last useful file edit: 17:22; three documentation files were substantively complete;
- killed: 20:10, after about 2h48m without further working-tree activity;
- task contract at termination: changes were not staged and `.git/A2A_COMMIT_MSG` did not exist;
- bridge stderr contained only the LSP warm notice and `[workflow] node edit started`—no node terminal;
- the requested `--out` file did not exist.

The task's named link check normally completes in under one second, but the evidence does not prove which
verification command, if any, was running. This is the more precise source for the earlier
`INC-VERIFY-STALL-2026-07-11` operator report.

## Incident B — Sol/max implement-arch

- rollout: `019f5979-3b44-7aa0-940c-54860e2ac968`, started 21:15:45;
- operator task: `b5xjhp2kr`;
- config/workflow: `stockTrading-impl-sol-max.toml` / `implement-arch`;
- a 54 KiB plan was written by 21:35, staged, and `.git/A2A_COMMIT_MSG` written by 21:36;
- killed: 22:03, after about 27m with the full repository-side implement contract already complete;
- process tree at termination: bridge 22714, shell 22698, and codex-acp descendants
  22724/22725/22726/22982; all were reported at 0% CPU;
- bridge stderr again ended at `[workflow] node edit started`; no node terminal or `--out` existed.

Repository-contract completion is useful takeover evidence, but it is not sufficient to mark the ACP
prompt or workflow node successful: the agent may still owe a response, verification may have failed
without updating the worktree, and stage/message conventions are repository-specific.

## Incident C — Sol/max implement-arch, plan-review round 1 fold

- rollout: `019f59b2-9d9e-7fc2-a932-8e789db5a697`, started 22:18:25;
- task/config: `bvtnr90zw`, `stockTrading-impl-sol-max.toml` / `implement-arch`;
- the plan's content, including its final dispositions section, was complete by about 22:33;
- killed: about 22:58, after roughly 25m post-content;
- task contract at termination: changes were not staged and `.git/A2A_COMMIT_MSG` did not exist;
- rollout mtime stopped around 22:57 and the bridge never emitted a node terminal or wrote `--out`.

## Incident D — Sol/max implement-arch, plan-review round 2 fold

- rollout: `019f59e7-20ea-7d13-a923-4cb0b27343a7`, started 23:15:47;
- task/config: `b30659rx5`, the same Sol/max `implement-arch` configuration;
- the plan content was complete by about 23:24;
- killed: about 00:03, after roughly 37m post-content;
- task contract at termination: changes were not staged and `.git/A2A_COMMIT_MSG` did not exist;
- rollout mtime stopped around 00:01 and no bridge node terminal or `--out` followed.

## Negative controls and effort correlation

An `implement-arch` run from rollout `019f595a-2baf-7a23-9237-e594857de78a` used the same config family,
performed a larger edit, emitted `node edit ok`, wrote `--out`, and exited normally between about 20:40
and 21:14. Its report explicitly stopped waiting after incomplete Cargo validation rather than waiting
indefinitely. This shows that the workflow shape and bridge build do not wedge every run, but it does not
isolate model effort, verification choice, macOS first-execution delay, or completion delivery as cause.

Across the operator's 12 completed runs that night, the observed separation was:

| Requested effort | Completed runs | Observed outcome |
|---|---:|---|
| `max` implement runs (Luna ×1, Sol ×3) | 4 | 4 post-work wedges |
| `high` implement runs (Sol ×3) | 3 | 3 clean exits |
| `xhigh` read-only reviews/designs | 5 | 5 clean exits |

A Terra/high run still in progress at report time is excluded from the completed-run totals. This small,
non-random sample makes requested `max` a strong reproduction variable and supersedes the earlier claim
that slow Cargo verification alone best explains the incidents: Incident A named only a sub-second link
check. It does **not** prove that model effort is the root cause. Task class, model, agent reasoning after
the last edit, ACP terminal delivery, and verification behavior remain confounded.

## Observed signature and unresolved alternatives

All four incidents share: useful work stopped, the ACP process tree remained present and quiet, bridge stderr
had a node start without a terminal, and end-only `--out` persistence lost the completed node report.
Plausible alternatives still include:

1. a healthy but silent long-running verification child;
2. a blocked or exited verification child whose waiter never completed;
3. a model/agent loop waiting indefinitely after repository work;
4. an ACP adapter losing or withholding terminal prompt completion;
5. a bridge workflow waiter or finalization path parked after terminal delivery.

The highest-value bounded reproduction is the same trivial implement contract twice against the same repo
and adapter: one run at `high`, one at `max`. Capture the resolved effort actually sent through ACP, rollout
terminal/update state, child tree, diagnostic phase, and bridge output-finalization state. A `high` clean
exit plus `max` post-work wedge would reproduce the correlation; either outcome in the opposite direction
would falsify perfect effort separation. Do not infer success merely from a staged diff or commit-message
file, and do not run `max` for routine reviews while this incident remains unresolved.

R2f must capture the owned child tree and command progress, rollout terminal/update state, ACP
`prompt_stream`/`prompt_finish` transitions, workflow-node terminal persistence, and `--out` finalization
separately. No timeout or automatic success inference is justified until those probes distinguish the
negative controls from the four failures.
