"""BLIND binary judge: grades a normalized findings block against ground
truth via the bridge's OWN `judge` workflow (a single kiro node -- see
evals/configs/eval-agents.toml). Ported IN SPIRIT from
prompts-skills-steering harness/judge.py; adapted:

  - invoked through `a2a-bridge run-workflow judge` (a real ACP agent turn,
    tools-off) instead of a direct `codex_cli` process wrapper -- the bridge
    itself is the transport, per the M3 spec's "the judge is ... run through
    `a2a-bridge run-workflow`".
  - output contract simplified to `{item_pass, defects: [{id, found}],
    false_findings, neutral_matched}` (`id`, not steering's `defect_id`; no
    separate `verdict_flagged` tag -- our review prompts do not emit a fixed
    APPROVE/REJECT token the way steering's did, so there is nothing to
    parse a flag out of). `item_pass` is asked of the judge directly (per
    the M3 spec's literal output contract) but then CROSS-CHECKED -- not
    blindly trusted -- against the judge's own found/false_findings answers
    via a MECHANICAL rule stated in the rubric (see `review_judge.md`): a
    mismatch is treated as a malformed response (retry once, then
    JudgeError), the same way steering treated any other bad field.
  - defect ids are closed-world: the judge may neither omit a ground-truth
    id nor invent one that isn't in truth. Either is a validation failure
    (retry once, then JudgeError) rather than steering's softer
    `judge_id_mismatch` bookkeeping -- see `_parse_and_validate`.
  - the judge NEVER shares a working directory with the caller: every
    attempt gets its OWN fresh, EMPTY `--session-cwd` (`tempfile.mkdtemp`),
    removed afterward -- a kiro turn that ignores the "no tools needed"
    prompt and tries to explore anyway has nothing real to read. (The ACP
    client also advertises no FS/terminal capability to begin with; this is
    defense in depth, not the only guard.)
"""
from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import yaml

_REQUIRED_KEYS = {"item_pass", "defects", "false_findings"}
_JSON_OBJECT_RE = re.compile(r"\{.*\}", re.DOTALL)


class JudgeError(Exception):
    """Raised when the judge cannot produce a valid grade after one retry."""


def load_truth(truth_path: str) -> dict:
    with open(truth_path) as f:
        return yaml.safe_load(f) or {}


def _truth_defect_ids(truth: dict) -> set[str]:
    return {str(d.get("id")) for d in (truth.get("defects") or []) if d.get("id") is not None}


def render_ground_truth(truth: dict) -> str:
    """Render truth.yaml as ground-truth text for the judge prompt.

    Ported IN SPIRIT from steering's `_render_ground_truth`; truth.yaml's own
    field names are UNCHANGED by this harness (`id`, `description`,
    `acceptable_match`, `reject_if`, `neutral_findings`, `clean_rationale`,
    `tempting_non_defects` -- see the M3 spec's Taskset section)."""
    defects = truth.get("defects") or []
    lines: list[str] = []
    if defects:
        lines.append("This item is SEEDED. Ground-truth defects:")
        for d in defects:
            lines.append(f"- id: {d.get('id')}")
            if d.get("description"):
                lines.append(f"  description: {d['description']}")
            if d.get("acceptable_match"):
                lines.append(f"  acceptable_match: {d['acceptable_match']}")
            if d.get("reject_if"):
                lines.append(f"  reject_if: {d['reject_if']}")
        neutral = truth.get("neutral_findings") or []
        if neutral:
            lines.append(
                "neutral_findings (true-but-out-of-scope; a finding matching one "
                "of these is NEITHER credited as a defect NOR a false finding -- "
                "count it in neutral_matched):"
            )
            for nf in neutral:
                lines.append(f"- {nf}")
    else:
        lines.append("This item is CLEAN. There are NO ground-truth defects.")
        if truth.get("clean_rationale"):
            lines.append(f"clean_rationale: {truth['clean_rationale']}")
        tempting = truth.get("tempting_non_defects") or []
        if tempting:
            lines.append(
                "tempting_non_defects (a finding matching one of these is a false finding):"
            )
            for t in tempting:
                lines.append(f"- {t}")
    return "\n".join(lines)


def build_judge_input(findings_block: str, truth: dict, rubric_text: str) -> str:
    """The complete judge task-spec BODY: rubric + rendered ground truth +
    the findings block. `task-type: freeform` (bridge-core's `task_spec.rs`
    -- zero required sections, the whole body IS the task) is used
    deliberately: a Description/Acceptance-Criteria ceremony doesn't fit a
    grading prompt. The `judge` WORKFLOW's own `prompt_file`
    (evals/rubrics/judge-task.md) is a bare `{{input}}` passthrough, so this
    text reaches the kiro agent completely unmodified."""
    return (
        "---\n"
        "task-type: freeform\n"
        "---\n"
        + rubric_text.rstrip()
        + "\n\nGROUND TRUTH:\n"
        + render_ground_truth(truth)
        + "\n\nFINDINGS BLOCK:\n"
        + findings_block.strip()
        + "\n"
    )


def _extract_json(text: str) -> dict:
    text = text.strip()
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        pass
    # Best-effort fallback: the agent wrapped the object in prose or a
    # markdown code fence despite the rubric's "no prose, no fence"
    # instruction -- grab the first {...} span and try again. Still a hard
    # failure (not a silent partial parse) if that also doesn't parse.
    m = _JSON_OBJECT_RE.search(text)
    if not m:
        raise ValueError("no JSON object found in judge output")
    return json.loads(m.group(0))


def _parse_and_validate(text: str, truth: dict) -> dict:
    """Strict parse + validate. Raising here is exactly what triggers
    `judge_review`'s retry-once-then-JudgeError contract -- every failure
    mode below (bad shape, wrong types, a defect id that doesn't match
    truth, or an `item_pass` that disagrees with the judge's own
    defects/false_findings) is treated identically: retry once, then give up
    loudly."""
    data = _extract_json(text)
    if not isinstance(data, dict):
        raise ValueError("judge output was JSON but not an object")
    missing = _REQUIRED_KEYS - set(data.keys())
    if missing:
        raise ValueError(f"judge output missing keys: {sorted(missing)}")

    defects = data.get("defects")
    if not isinstance(defects, list):
        raise ValueError("judge 'defects' must be a list")
    id_list: list[str] = []
    for i, d in enumerate(defects):
        if not isinstance(d, dict):
            raise ValueError(f"judge 'defects'[{i}] is not an object")
        if not isinstance(d.get("id"), str):
            raise ValueError(f"judge 'defects'[{i}] missing string 'id'")
        if not isinstance(d.get("found"), bool):
            raise ValueError(f"judge 'defects'[{i}] missing bool 'found'")
        id_list.append(d["id"])
    if len(id_list) != len(set(id_list)):
        raise ValueError("judge 'defects' contains a duplicate id")

    truth_ids = _truth_defect_ids(truth)
    seen_ids = set(id_list)
    missing_ids = truth_ids - seen_ids
    if missing_ids:
        raise ValueError(f"judge 'defects' omitted ground-truth id(s): {sorted(missing_ids)}")
    extra_ids = seen_ids - truth_ids
    if extra_ids:
        # Closed-world: for a CLEAN item truth_ids is empty, so ANY entry at
        # all lands here too -- "defects must have exactly one entry per
        # ground-truth defect id (zero entries if the item is CLEAN)".
        raise ValueError(f"judge 'defects' included id(s) not in ground truth: {sorted(extra_ids)}")

    if not isinstance(data.get("false_findings"), int) or isinstance(data.get("false_findings"), bool):
        raise ValueError("judge 'false_findings' must be an integer")
    neutral_matched = data.get("neutral_matched", 0)
    if not isinstance(neutral_matched, int) or isinstance(neutral_matched, bool):
        raise ValueError("judge 'neutral_matched' must be an integer")
    data["neutral_matched"] = neutral_matched

    if not isinstance(data.get("item_pass"), bool):
        raise ValueError("judge 'item_pass' must be a bool")

    # Mechanical cross-check (the rubric asks the judge to compute item_pass
    # THIS way, deliberately, so its job is pattern-matching, not fuzzy
    # self-aggregation): SEEDED -> all truth defects found; CLEAN -> zero
    # false findings.
    if truth_ids:
        expected_pass = all(d.get("found") for d in defects if d.get("id") in truth_ids)
    else:
        expected_pass = data["false_findings"] == 0
    if data["item_pass"] != expected_pass:
        raise ValueError(
            f"judge 'item_pass'={data['item_pass']} is inconsistent with its own "
            f"defects/false_findings (expected {expected_pass} per the rubric's "
            f"mechanical rule)"
        )
    return data


def judge_review(
    findings_block: str,
    truth: dict,
    *,
    bridge_bin,
    bridge_config,
    rubric_path=None,
    rubric_text: str | None = None,
    timeout_s: int = 300,
) -> dict:
    """Grade `findings_block` against `truth` via
    `a2a-bridge run-workflow judge`. Retries once on a malformed/failed
    response; raises `JudgeError` on a second failure.

    Every attempt gets its OWN fresh, empty scratch `--session-cwd`, removed
    afterward in a `finally` -- see the module docstring's isolation note.
    """
    if rubric_text is None:
        if rubric_path is None:
            raise ValueError("judge_review requires rubric_text or rubric_path")
        rubric_text = Path(rubric_path).read_text()

    prompt = build_judge_input(findings_block, truth, rubric_text)

    last_err: Exception | None = None
    for _attempt in range(2):
        scratch = Path(tempfile.mkdtemp(prefix="a2a-bridge-eval-judge-"))
        try:
            proc = subprocess.run(
                [
                    str(bridge_bin),
                    "run-workflow",
                    "judge",
                    "--input",
                    "-",
                    "--session-cwd",
                    str(scratch),
                    "--config",
                    str(bridge_config),
                ],
                input=prompt,
                capture_output=True,
                text=True,
                timeout=timeout_s,
            )
            if proc.returncode != 0:
                tail = (proc.stdout + proc.stderr).strip()
                raise ValueError(f"judge workflow exited {proc.returncode}: {tail[-500:]}")
            return _parse_and_validate(proc.stdout, truth)
        except (subprocess.TimeoutExpired, ValueError, json.JSONDecodeError) as e:
            last_err = e
            continue
        finally:
            shutil.rmtree(scratch, ignore_errors=True)
    raise JudgeError(f"judge failed after retry: {last_err}")


def main(argv=None) -> int:
    """Standalone CLI: grade one already-produced findings file against one
    truth.yaml. Mainly for ad hoc re-grading/debugging outside the full
    `runner.py` pipeline -- `runner.py` calls `judge_review` directly as a
    library function for the real matrix run."""
    import argparse

    from harness import normalize as normalize_mod

    p = argparse.ArgumentParser(prog="harness.judge")
    p.add_argument("findings_path", type=Path)
    p.add_argument("truth_path", type=Path)
    p.add_argument("--bridge-bin", type=Path, required=True)
    p.add_argument("--bridge-config", type=Path, required=True)
    p.add_argument("--rubric", type=Path, required=True)
    p.add_argument("--timeout", type=int, default=300)
    args = p.parse_args(argv)

    findings = normalize_mod.normalize_findings(args.findings_path.read_text())
    truth = load_truth(str(args.truth_path))
    try:
        result = judge_review(
            findings,
            truth,
            bridge_bin=args.bridge_bin,
            bridge_config=args.bridge_config,
            rubric_path=args.rubric,
            timeout_s=args.timeout,
        )
    except JudgeError as e:
        print(f"judge: {e}", file=sys.stderr)
        return 1
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
