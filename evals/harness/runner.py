#!/usr/bin/env python3
"""M3 eval pipeline entry point: drives `a2a-bridge run-workflow` per
(cell x item), grades each completed call with the blind judge, and renders
the report. See docs/superpowers/specs/2026-07-04-m3-eval-harness.md.

Ported IN SPIRIT from prompts-skills-steering harness/run.py (stale-results
guard, per-arm/here per-cell integrity bookkeeping, budget gate BEFORE the
expensive work, nonzero exit whenever a judge_error occurred) rebuilt around
`a2a-bridge run-workflow` instead of promptfoo, and around 3 CELLS instead of
2 arms.

Usage:
    uv run python -m harness.runner --smoke
    uv run python -m harness.runner --cells duo,codex-solo,claude-solo
    uv run python -m harness.runner --dry-run
"""
from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import subprocess
import sys
import time
from pathlib import Path

from harness import config as config_mod
from harness import judge as judge_mod
from harness import normalize as normalize_mod
from harness import report as report_mod


# --------------------------------------------------------------------------- #
# Task-spec generation.
# --------------------------------------------------------------------------- #
def build_task_spec(item: config_mod.ManifestItem) -> str:
    """Render one taskset item's `context.md` + `diff.patch` into a
    `code-review` task-spec body. `code-review`'s schema (bridge-core's
    `task_spec.rs`, `REVIEW_SECTIONS`) requires only `Description` +
    `Acceptance Criteria` -- see the M3 spec's "Generated task-specs embed
    context.md + the fenced diff.patch in the body". `truth.yaml` is never
    read here -- only context.md/diff.patch reach the executor."""
    context = item.context_path.read_text().strip()
    diff = item.diff_path.read_text().rstrip("\n")
    return (
        "---\n"
        "task-type: code-review\n"
        "---\n"
        f"# Review: {item.id}\n"
        "\n"
        "## Description\n"
        f"{context}\n"
        "\n"
        "```diff\n"
        f"{diff}\n"
        "```\n"
        "\n"
        "## Acceptance Criteria\n"
        "- Identify any correctness, safety, or design defects introduced by this diff.\n"
        "- For each finding, state what is wrong, why it matters, and cite its location.\n"
        "- If no defects are found, say so explicitly.\n"
    )


# --------------------------------------------------------------------------- #
# One (cell, item) `run-workflow` invocation.
# --------------------------------------------------------------------------- #
def run_workflow(
    bridge_bin: Path,
    workflow_id: str,
    task_spec_text: str,
    session_cwd: Path,
    bridge_config: Path,
    out_path: Path,
    timeout_s: int,
) -> dict:
    """Invoke `a2a-bridge run-workflow` for one (cell, item). Returns a
    `calls/*.json`-shaped metadata dict. Never raises on a workflow/agent
    failure or timeout -- those are recorded in the dict (`ok: false`) so one
    bad item cannot crash the whole matrix; only a genuine caller/setup bug
    (e.g. a missing fixture dir) raises.

    `--session-cwd` is resolved to an ABSOLUTE path and asserted to exist
    before the call: `SessionCwd::parse` (bridge-core) rejects a relative
    path outright, so a caller bug here would otherwise surface as an opaque
    bridge-side error deep in a subprocess instead of a clear one here.
    """
    session_cwd = Path(session_cwd).resolve()
    if not session_cwd.is_absolute():
        raise AssertionError(f"session_cwd did not resolve absolute: {session_cwd}")
    if not session_cwd.is_dir():
        raise config_mod.ConfigError(f"fixture dir does not exist: {session_cwd}")

    cmd = [
        str(bridge_bin),
        "run-workflow",
        workflow_id,
        "--input",
        "-",
        "--session-cwd",
        str(session_cwd),
        "--config",
        str(bridge_config),
        "--out",
        str(out_path),
    ]
    started = time.time()
    timed_out = False
    returncode: int | None
    stderr = ""
    try:
        proc = subprocess.run(
            cmd, input=task_spec_text, capture_output=True, text=True, timeout=timeout_s
        )
        returncode = proc.returncode
        stderr = proc.stderr or ""
    except subprocess.TimeoutExpired as e:
        timed_out = True
        returncode = None
        stderr = e.stderr if isinstance(e.stderr, str) else ""
    elapsed = time.time() - started

    output_text = out_path.read_text() if out_path.is_file() else None
    ok = (
        not timed_out
        and returncode == 0
        and output_text is not None
        and not normalize_mod.is_node_failure(output_text)
    )
    return {
        "workflow_id": workflow_id,
        "session_cwd": str(session_cwd),
        "returncode": returncode,
        "timed_out": timed_out,
        "elapsed_s": round(elapsed, 3),
        "ok": ok,
        "stderr_tail": "\n".join(stderr.strip().splitlines()[-20:]) if stderr else "",
        "out_path": str(out_path),
    }


# --------------------------------------------------------------------------- #
# Judge one completed call.
# --------------------------------------------------------------------------- #
def judge_call(
    bridge_bin: Path,
    bridge_config: Path,
    rubric_text: str,
    truth_path: Path,
    findings_text: str,
    timeout_s: int,
) -> dict:
    """Grade one item's findings via the blind `judge` workflow. Always
    returns a dict with `judge_error` present (False on success) -- a
    `JudgeError` is caught here (not left to propagate) so one bad grade
    cannot crash the whole matrix; the overall run still exits nonzero
    at the end if any `judge_error` occurred (see `main`)."""
    truth = judge_mod.load_truth(str(truth_path))
    try:
        result = judge_mod.judge_review(
            findings_text,
            truth,
            bridge_bin=bridge_bin,
            bridge_config=bridge_config,
            rubric_text=rubric_text,
            timeout_s=timeout_s,
        )
        result["judge_error"] = False
        return result
    except judge_mod.JudgeError as e:
        return {
            "judge_error": True,
            "judge_error_detail": str(e),
            "item_pass": False,
            "defects": [],
            "false_findings": 0,
            "neutral_matched": 0,
        }


# --------------------------------------------------------------------------- #
# Optional tracing hook (opik, or a jsonl fallback) -- NOT on the critical
# path; must never fail a real run.
# --------------------------------------------------------------------------- #
def _trace(out_dir: Path, event: str, **fields) -> None:
    """Best-effort tracing. Ported spirit of steering's opik hook: lazy
    import opik ONLY when OPIK_API_KEY is set; otherwise append a JSON line
    to `trace.jsonl` in the results dir. Never raises."""
    record = {"event": event, "ts": time.time(), **fields}
    if os.environ.get("OPIK_API_KEY"):
        try:
            import opik  # noqa: PLC0415 -- intentionally lazy/optional; not a hard dep

            # NOT exercised against a live opik project in v1 (see the M3
            # spec's "if it costs more than the trivial hook, defer it") --
            # this call shape may need adjustment against a real opik SDK
            # version before it is relied on.
            opik.track(**record)
            return
        except Exception:
            pass
    try:
        with open(out_dir / "trace.jsonl", "a") as f:
            f.write(json.dumps(record, default=str) + "\n")
    except Exception:
        pass


# --------------------------------------------------------------------------- #
# Stale-results guard (ported spirit of steering's `_check_stale_results_dir`).
# --------------------------------------------------------------------------- #
def _guard_stale_dir(d: Path, force: bool) -> None:
    stale = sorted(d.glob("*.json"))
    if not stale:
        return
    if not force:
        raise config_mod.ConfigError(
            f"refusing to run: {len(stale)} stale result file(s) already exist under "
            f"{d} from a previous run. Delete them yourself and re-run, or pass "
            f"--force to clear them first."
        )
    for p in stale:
        p.unlink()
    for p in sorted(d.glob("*.md")):
        p.unlink()


# --------------------------------------------------------------------------- #
# Run-dir manifest (C1 regression discipline: bridge version + cell configs
# + taskset id stamped into every run dir).
# --------------------------------------------------------------------------- #
def _write_run_manifest(
    out_dir: Path, *, cells: list[config_mod.Cell], taskset_id: str, bridge_bin: Path, bridge_config: Path
) -> None:
    git_sha = None
    try:
        proc = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=str(config_mod.REPO_ROOT),
            capture_output=True,
            text=True,
            timeout=5,
        )
        if proc.returncode == 0:
            git_sha = proc.stdout.strip()
    except Exception:
        pass

    help_head = None
    try:
        proc = subprocess.run([str(bridge_bin), "--help"], capture_output=True, text=True, timeout=5)
        lines = proc.stdout.splitlines()
        help_head = lines[0] if lines else None
    except Exception:
        pass

    manifest = {
        "bridge_git_sha": git_sha,
        "bridge_help_head": help_head,
        "bridge_bin": str(bridge_bin),
        "bridge_config": str(bridge_config),
        "taskset_id": taskset_id,
        "cells": [
            {"id": c.id, "workflow_id": c.workflow_id, "authorship_agents": list(c.authorship_agents)}
            for c in cells
        ],
        "judge_agent_id": config_mod.JUDGE_AGENT_ID,
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    (out_dir / "run_manifest.json").write_text(json.dumps(manifest, indent=2))


# --------------------------------------------------------------------------- #
# The full matrix: run -> judge -> report.
# --------------------------------------------------------------------------- #
def run_matrix(
    *,
    cells: list[config_mod.Cell],
    items: list[config_mod.ManifestItem],
    bridge_bin: Path,
    bridge_config: Path,
    rubric_path: Path,
    out_dir: Path,
    concurrency: int,
    timeout_s: int,
    judge_timeout_s: int,
    skip_judge: bool,
    force: bool,
    taskset_id: str,
) -> int:
    calls_dir = out_dir / "calls"
    judge_dir = out_dir / "judge"
    calls_dir.mkdir(parents=True, exist_ok=True)
    judge_dir.mkdir(parents=True, exist_ok=True)

    _guard_stale_dir(calls_dir, force)
    _guard_stale_dir(judge_dir, force)

    rubric_text = rubric_path.read_text()

    def _one_call(cell: config_mod.Cell, item: config_mod.ManifestItem) -> tuple[str, dict]:
        key = f"{cell.id}-{item.id}"
        out_path = calls_dir / f"{key}.md"
        task_spec = build_task_spec(item)
        meta = run_workflow(
            bridge_bin, cell.workflow_id, task_spec, item.fixture_dir, bridge_config, out_path, timeout_s
        )
        meta.update({"cell": cell.id, "item": item.id, "seeded": item.seeded, "truth_path": str(item.truth_path)})
        (calls_dir / f"{key}.json").write_text(json.dumps(meta, indent=2))
        _trace(out_dir, "call_done", key=key, ok=meta["ok"], elapsed_s=meta["elapsed_s"])
        return key, meta

    work = [(cell, item) for cell in cells for item in items]
    results: dict[str, dict] = {}
    print(f"[runner] running {len(work)} call(s) at concurrency={concurrency}...", flush=True)
    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as pool:
        futures = {pool.submit(_one_call, cell, item): (cell, item) for cell, item in work}
        for fut in concurrent.futures.as_completed(futures):
            cell, item = futures[fut]
            key, meta = fut.result()
            results[key] = meta
            status = "ok" if meta["ok"] else "FAILED"
            print(f"[runner] {key}: {status} ({meta['elapsed_s']}s)", flush=True)

    judge_errors = 0
    if not skip_judge:
        print(f"[runner] judging {len(work)} call(s)...", flush=True)
        for cell in cells:
            for item in items:
                key = f"{cell.id}-{item.id}"
                meta = results[key]
                out_path = Path(meta["out_path"])
                raw = out_path.read_text() if out_path.is_file() else ""
                findings_text = normalize_mod.normalize_findings(raw)
                jr = judge_call(bridge_bin, bridge_config, rubric_text, item.truth_path, findings_text, judge_timeout_s)
                jr.update(
                    {
                        "cell": cell.id,
                        "item": item.id,
                        "seeded": item.seeded,
                        "truth_path": str(item.truth_path),
                        "normalized_block": findings_text,
                        "call_ok": meta["ok"],
                    }
                )
                (judge_dir / f"{key}.json").write_text(json.dumps(jr, indent=2))
                if jr["judge_error"]:
                    judge_errors += 1
                    print(f"[runner] judge {key}: ERROR -- {jr.get('judge_error_detail')}", file=sys.stderr, flush=True)
                else:
                    print(f"[runner] judge {key}: {'pass' if jr['item_pass'] else 'fail'}", flush=True)
                _trace(out_dir, "judge_done", key=key, judge_error=jr["judge_error"])

        report_mod.render(out_dir, taskset_id=taskset_id, cells=[c.id for c in cells], n_items=len(items))
        print(f"[runner] report written to {out_dir}/report.md", flush=True)

    if judge_errors:
        print(f"[runner] {judge_errors} judge_error(s) -- run is not clean.", file=sys.stderr, flush=True)
        return 1
    return 0


# --------------------------------------------------------------------------- #
# CLI.
# --------------------------------------------------------------------------- #
def build_arg_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="harness.runner",
        description="Run the M3 code-review eval matrix (cells x taskset items), then grade + report.",
    )
    p.add_argument("--taskset", type=Path, default=config_mod.DEFAULT_TASKSET_DIR)
    p.add_argument("--bridge-config", type=Path, default=config_mod.DEFAULT_BRIDGE_CONFIG)
    p.add_argument("--bridge-bin", type=Path, default=config_mod.DEFAULT_BRIDGE_BIN)
    p.add_argument("--rubric", type=Path, default=config_mod.DEFAULT_RUBRIC_PATH)
    p.add_argument(
        "--out", type=Path, default=None, help="results dir (default: evals/results/<timestamp>-<taskset-id>)"
    )
    p.add_argument("--cells", default="duo,codex-solo,claude-solo", help="comma-separated cell ids to run")
    p.add_argument("--smoke", action="store_true", help="shorthand for --cells duo --limit 3")
    p.add_argument("--limit", type=int, default=None, help="cap the number of taskset items")
    p.add_argument("--cap", type=int, default=120, help="budget gate: max projected turns (workflow + judge calls)")
    p.add_argument("--timeout", type=int, default=900, help="per-item run-workflow wall timeout (seconds)")
    p.add_argument("--judge-timeout", type=int, default=300, help="per-item judge wall timeout (seconds)")
    p.add_argument("--concurrency", type=int, default=2, help="subscription-quota politeness -- do not raise in production")
    p.add_argument("--skip-judge", action="store_true", help="run only the workflow calls; skip grading + report")
    p.add_argument("--force", action="store_true", help="clear stale calls/judge files in --out before running")
    p.add_argument(
        "--dry-run", action="store_true", help="print the planned matrix + budget projection and exit without touching agents"
    )
    p.add_argument("--allow-same-family-judge", action="store_true")
    return p


def main(argv=None) -> int:
    args = build_arg_parser().parse_args(argv)

    cell_ids = ["duo"] if args.smoke else [c.strip() for c in args.cells.split(",") if c.strip()]
    limit = 3 if args.smoke else args.limit

    try:
        cells = [config_mod.CELLS[c] for c in cell_ids]
    except KeyError as e:
        print(f"runner: unknown cell {e.args[0]!r}; known cells: {sorted(config_mod.CELLS)}", file=sys.stderr)
        return 2

    try:
        config_mod.validate_same_family_judge(cells, config_mod.JUDGE_AGENT_ID, allow=args.allow_same_family_judge)
        bridge_bin = config_mod.resolve_bridge_bin(args.bridge_bin)
        bridge_config = config_mod.resolve_bridge_config(args.bridge_config)
        rubric_path = config_mod.resolve_rubric(args.rubric)
        items = config_mod.load_manifest(args.taskset, limit=limit)
    except config_mod.ConfigError as e:
        print(f"runner: {e}", file=sys.stderr)
        return 2

    projected = config_mod.project_turn_count(cells, len(items), judge=not args.skip_judge)
    print(f"[runner] planned: {len(cells)} cell(s) x {len(items)} item(s) = {projected} projected turn(s) (cap {args.cap})")
    try:
        config_mod.budget_gate(projected, args.cap)
    except config_mod.BudgetExceeded as e:
        print(f"runner: {e}", file=sys.stderr)
        return 3

    taskset_id = Path(args.taskset).name
    out_dir = Path(args.out) if args.out else (
        config_mod.DEFAULT_RESULTS_ROOT / f"{time.strftime('%Y-%m-%d-%H%M%S')}-{taskset_id}"
    )

    if args.dry_run:
        print(f"[runner] dry-run: would write to {out_dir}")
        for cell in cells:
            for item in items:
                print(f"  {cell.id}-{item.id}  (workflow={cell.workflow_id}, fixture={item.fixture_dir})")
        return 0

    out_dir.mkdir(parents=True, exist_ok=True)
    _write_run_manifest(out_dir, cells=cells, taskset_id=taskset_id, bridge_bin=bridge_bin, bridge_config=bridge_config)

    return run_matrix(
        cells=cells,
        items=items,
        bridge_bin=bridge_bin,
        bridge_config=bridge_config,
        rubric_path=rubric_path,
        out_dir=out_dir,
        concurrency=args.concurrency,
        timeout_s=args.timeout,
        judge_timeout_s=args.judge_timeout,
        skip_judge=args.skip_judge,
        force=args.force,
        taskset_id=taskset_id,
    )


if __name__ == "__main__":
    raise SystemExit(main())
