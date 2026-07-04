"""Metrics PORTED from prompts-skills-steering harness/metrics.py
(stdlib-only: no numpy/scipy). See that file's own module docstring for the
original accounting philosophy (logical tokens vs USD cost reported
separately; judge_error rows excluded from pass/confusion but surfaced as a
count).

Adapted from steering's baseline/treatment ARM vocabulary to this harness's
3-CELL vocabulary (duo / codex-solo / claude-solo -- see
`evals.harness.config`), and from steering's promptfoo-derived per-call
token/cost rows to the fields this harness's `calls/*.json` + `judge/*.json`
rows actually carry (there is no token/cost accounting available here: an
`a2a-bridge run-workflow` invocation does not currently surface
`NodeFinished.usage` on stdout/exit).

Ported UNCHANGED in behavior: `wilson_ci`, `mcnemar_p` (only parameter names
generalized -- the math is identical and symmetric in its arguments either
way). Ported and ADAPTED (see each docstring for the exact delta):
`confusion`, `paired_flips`, `pass_rate`. Deliberately NOT ported:
`token_totals` / `judge_token_totals` / `delta` / `adherence` /
`triggering_metrics` / steering's `flags` (tier mixing, cost-adjusted
verdicts, negative controls, adherence directives) -- this harness has no
data source for any of those concepts in v1.
"""
from __future__ import annotations

import glob
import json
import math
import os

Z_95 = 1.959963984540054  # 97.5th percentile of the standard normal


class IntegrityError(Exception):
    """Raised when one or more per-item result JSON files cannot be parsed.

    Collected across the whole glob and raised once (with every offending
    path) so a single truncated file cannot masquerade as a clean, smaller
    run. Ported verbatim from steering's `metrics.py`.
    """


def _load_json_glob(pattern: str) -> list[dict]:
    rows = []
    bad = []
    for p in sorted(glob.glob(pattern)):
        try:
            with open(p) as f:
                rows.append(json.load(f))
        except (json.JSONDecodeError, OSError, UnicodeDecodeError) as e:
            bad.append(f"{p}: {e}")
    if bad:
        raise IntegrityError(
            "unparseable per-item result file(s):\n  " + "\n  ".join(bad)
        )
    return rows


def load_rows(results_dir) -> tuple[list[dict], list[dict]]:
    """Load (calls, judges) row lists from a results dir's `calls/` and
    `judge/` subdirs. Ported from steering's `load_rows` -- same
    glob-everything-then-raise-once integrity contract."""
    calls = _load_json_glob(os.path.join(str(results_dir), "calls", "*.json"))
    judges = _load_json_glob(os.path.join(str(results_dir), "judge", "*.json"))
    return calls, judges


def _is_scoreable(r: dict) -> bool:
    """A judge row contributes to pass/confusion/recall/flip metrics iff it
    carries a real judge decision. Two kinds of row are EXCLUDED, for two
    distinct reasons, and are never counted as either a pass or a fail:

      - `judge_error` -- the judge itself failed to produce a valid grade
        after a retry (a harness/judge failure).
      - `call_failed` -- the REVIEWER call failed/timed-out/emptied, so the
        judge was deliberately never invoked (a reviewer/agent failure). This
        is what `harness.normalize`'s own docstring means by "callers that
        want to skip judging a failed call": conflating a reviewer crash with
        a real judge verdict would either mislabel it a judge_error (if the
        mechanical clean-item cross-check forces a raise) or launder it into a
        clean TN (if the judge obeys the marker rule) -- both wrong.

    Counting either separately (never folded into k/n or TP/FP/TN/FN) keeps a
    partially-broken run from masquerading as a smaller clean one."""
    return not r.get("judge_error") and not r.get("call_failed")


# --------------------------------------------------------------------------- #
# Pass rate + Wilson interval.
# --------------------------------------------------------------------------- #
def wilson_ci(k: int, n: int, z: float = Z_95) -> tuple[float, float]:
    """Wilson score 95% CI for k successes out of n. Returns (low, high).
    Ported VERBATIM from steering's `metrics.py`."""
    if n == 0:
        return (0.0, 0.0)
    p = k / n
    z2 = z * z
    denom = 1.0 + z2 / n
    center = (p + z2 / (2 * n)) / denom
    margin = (z / denom) * math.sqrt(p * (1 - p) / n + z2 / (4 * n * n))
    return (max(0.0, center - margin), min(1.0, center + margin))


def pass_rate(rows: list[dict], cell: str) -> dict:
    """Item pass rate for one cell. Excludes both judge_error AND call_failed
    rows from k/n (see `_is_scoreable`) and reports each excluded count
    separately as `judge_errors` / `call_failures`.

    ADAPTED from steering's `pass_rate`: `arm` -> `cell`, tier-mixing guard
    dropped (this harness runs exactly one taskset per results dir, so there
    is no second axis to accidentally mix); `call_failures` added for the
    MAJOR-1 skip-failed-reviewer-calls path."""
    cell_rows = [r for r in rows if r.get("cell") == cell]
    judge_errors = sum(1 for r in cell_rows if r.get("judge_error"))
    # call_failed rows are counted only when they are NOT also judge_error
    # (a call_failed row never sets judge_error, but guard the arithmetic so
    # the two counts can never double-count the same excluded row).
    call_failures = sum(1 for r in cell_rows if r.get("call_failed") and not r.get("judge_error"))
    scored = [r for r in cell_rows if _is_scoreable(r)]
    n = len(scored)
    k = sum(1 for r in scored if r.get("item_pass"))
    rate = (k / n) if n else 0.0
    return {
        "k": k,
        "n": n,
        "rate": rate,
        "wilson_ci": wilson_ci(k, n),
        "judge_errors": judge_errors,
        "call_failures": call_failures,
    }


# --------------------------------------------------------------------------- #
# Confusion matrix (item-level pass/fail + defect-level recall).
# --------------------------------------------------------------------------- #
def confusion(rows: list[dict], cell: str) -> dict:
    """Item-level TP/FP/TN/FN + base rate, defect-level recall, false
    findings, for one cell. Excludes judge_error AND call_failed rows (see
    `_is_scoreable`).

    ADAPTED from steering's `confusion`: steering derived "flagged" from a
    literal APPROVE/REJECT tag the reviewed artifact itself emitted
    (`verdict_flagged`). This harness's review prompts do not emit a fixed
    verdict token (they end in a free-form "one-line verdict"), so there is
    no such field on a `judge/*.json` row -- `flagged` is instead DERIVED
    here, per row, as "the review raised at least one thing": at least one
    ground-truth defect found, OR `false_findings > 0`. Item level: TP =
    seeded & flagged; FN = seeded & !flagged; FP = clean & flagged; TN =
    clean & !flagged.

    `defect_recall` is anchored to GROUND TRUTH, never to the judge's own id
    list, exactly as steering's version: the denominator is the seeded
    defects from truth (via each row's own `defects` list, which always has
    exactly one entry per truth id -- see `harness.judge._parse_and_validate`,
    which REJECTS any judge response that invents an id not in truth or omits
    one that is, rather than tallying a `judge_id_mismatch` after the fact
    the way steering's judge did). This is what "PRIMARY metric = defect-level
    catch-rate ... never inflated by judge-hallucinated matches" (M3 spec)
    means concretely: a hallucinated id can never even reach this function,
    because the judge step already refused to accept it.

    Also reports `false_findings_clean_total`, the false-finding count
    restricted to CLEAN items only (the M3 spec's secondary metric
    "false-finding count on clean items (FRAGILE at n=4 clean)") as a
    breakdown of the overall `false_findings_total`.
    """
    cell_rows = [r for r in rows if r.get("cell") == cell and _is_scoreable(r)]
    tp = fp = tn = fn = 0
    found = total = false_findings = false_findings_clean = neutral_matched = 0
    for r in cell_rows:
        seeded = bool(r.get("seeded"))
        defects = r.get("defects") or []
        ff = int(r.get("false_findings", 0) or 0)
        flagged = ff > 0 or any(d.get("found") for d in defects)
        if seeded and flagged:
            tp += 1
        elif seeded and not flagged:
            fn += 1
        elif not seeded and flagged:
            fp += 1
        else:
            tn += 1
        if seeded:
            truth_ids = {d.get("id") for d in defects if d.get("id") is not None}
            total += len(truth_ids)
            found += sum(1 for d in defects if d.get("found"))
        else:
            false_findings_clean += ff
        false_findings += ff
        neutral_matched += int(r.get("neutral_matched", 0) or 0)
    n = len(cell_rows)
    positives = tp + fn
    return {
        "tp": tp,
        "fp": fp,
        "tn": tn,
        "fn": fn,
        "n": n,
        "base_rate": (positives / n) if n else 0.0,
        "defect_recall": {
            "found": found,
            "total": total,
            "rate": (found / total) if total else 0.0,
        },
        "false_findings_total": false_findings,
        "false_findings_clean_total": false_findings_clean,
        "neutral_matched_total": neutral_matched,
    }


# --------------------------------------------------------------------------- #
# Paired flip table + exact McNemar p-value.
# --------------------------------------------------------------------------- #
def paired_flips(rows: list[dict], cell_a: str, cell_b: str) -> dict:
    """Join two cells' judge rows on item id and classify the 2x2 flips.

    ADAPTED from steering's `paired_flips`: generalized from a hardcoded
    baseline/treatment pair to two CALLER-NAMED cells, since this harness
    compares 3 cells pairwise (duo vs codex-solo, duo vs claude-solo) rather
    than a single fixed arm pair. Items with a judge_error OR a call_failed
    (skipped-judge) row in either cell, or missing from a cell entirely, are
    excluded (a flip needs a real judge verdict in both cells)."""
    by_item: dict[str, dict[str, dict]] = {}
    for r in rows:
        cell = r.get("cell")
        if cell not in (cell_a, cell_b):
            continue
        by_item.setdefault(r["item"], {})[cell] = r

    both_pass = both_fail = only_a = only_b = 0
    for item_id, cells in by_item.items():
        a = cells.get(cell_a)
        b = cells.get(cell_b)
        if a is None or b is None:
            continue
        # A flip needs a real judge verdict in BOTH cells -- exclude the pair
        # if either side is a judge_error or a call_failed (skipped-judge) row.
        if not _is_scoreable(a) or not _is_scoreable(b):
            continue
        ap = bool(a.get("item_pass"))
        bp = bool(b.get("item_pass"))
        if ap and bp:
            both_pass += 1
        elif not ap and not bp:
            both_fail += 1
        elif ap and not bp:
            only_a += 1
        else:
            only_b += 1
    return {
        "cell_a": cell_a,
        "cell_b": cell_b,
        "both_pass": both_pass,
        "both_fail": both_fail,
        "only_a": only_a,
        "only_b": only_b,
    }


def mcnemar_p(only_a: int, only_b: int) -> float:
    """Exact two-sided McNemar test p-value on the discordant-pair counts
    from `paired_flips` (only_a, only_b).

    This is the binomial sign test: under the null (no directional effect,
    each discordant pair is equally likely to flip either way),
    n=only_a+only_b ~ Binomial(n, 0.5) counts how many favored one side.
    p = 2 * the smaller of the two one-sided binomial tail probabilities,
    capped at 1.0. Implemented with `math.comb` only -- no scipy dependency.

    n=0 (no discordant pairs at all) returns 1.0: zero evidence of a
    directional difference is the maximally-non-significant case, not an
    undefined one.

    Ported VERBATIM from steering's `mcnemar_p` (param names generalized from
    only_baseline/only_treatment to only_a/only_b -- the math is unchanged
    and symmetric in its arguments either way). Per the M3 spec, this value
    is REPORTED but demoted to a paired-flips display, never a pass/fail
    criterion -- see `report.py`.
    """
    n = only_a + only_b
    if n == 0:
        return 1.0
    lo, hi = min(only_a, only_b), max(only_a, only_b)

    def _tail_le(k: int) -> float:
        return sum(math.comb(n, i) for i in range(0, k + 1)) * (0.5**n)

    p_lo = _tail_le(lo)
    p_hi = 1.0 - _tail_le(hi - 1)
    return min(2.0 * min(p_lo, p_hi), 1.0)
