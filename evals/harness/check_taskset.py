#!/usr/bin/env python3
"""Validate a review-seeded taskset dir.

PORTED from prompts-skills-steering scripts/check_taskset.py; ADAPTED for
this harness's item shape:

  - every item additionally carries a `fixture/` dir (a tiny buildable Rust
    crate with the defect APPLIED -- this repo's reviews are read-only-TOOLS
    + `--session-cwd`, not steering's tool-free inline-only reviews, so
    items must provide a navigable fixture as well as the inline diff).
  - every SEEDED item's defects each carry a per-defect `catchability` field
    (a one-line "catchable-from-diff because..." human note) -- the M3
    spec's fairness gate for read-only-reviewable risky defect classes
    (cancellation-unsafety, stale-lock/double-release, unbounded growth):
    the diff must expose the invariant so a read-only reviewer can catch it
    without executing tests, and that judgment call is recorded per defect.
  - `truth.yaml` lives at the item root (unchanged from steering), NOT
    inside `fixture/` -- this is load-bearing, not cosmetic: keeping it out
    of the fixture dir means `--session-cwd` navigation over the fixture can
    never leak the ground truth to an executor.

Exit codes: 0 = OK. 2 = the taskset dir (or its manifest.yaml) does not
exist -- distinct from 1 so a caller can tell "nothing to check yet" (corpus
not authored yet) apart from "checked it and it's broken". 1 = one or more
validation errors within an existing taskset dir.
"""
from __future__ import annotations

import argparse
import pathlib
import subprocess
import sys

import yaml

DEFECT_FIELDS = [
    "id",
    "defect_class",
    "location",
    "hunk_lines",
    "description",
    "root_cause",
    "bad_behavior",
    "minimal_trigger",
    "acceptable_match",
    "reject_if",
    "catchability",
]


def _build_fixture(fixture_dir: pathlib.Path, item_id: str, errors: list[str]) -> None:
    if not (fixture_dir / "Cargo.toml").is_file():
        errors.append(f"{item_id}: fixture/ missing Cargo.toml")
        return
    try:
        proc = subprocess.run(
            ["cargo", "build", "-j", "1"],
            cwd=str(fixture_dir),
            capture_output=True,
            text=True,
            timeout=600,
        )
    except (OSError, subprocess.TimeoutExpired) as e:
        errors.append(f"{item_id}: fixture/ `cargo build -j 1` could not run: {e}")
        return
    if proc.returncode != 0:
        tail = "\n".join((proc.stdout + proc.stderr).splitlines()[-30:])
        errors.append(f"{item_id}: fixture/ `cargo build -j 1` failed:\n{tail}")


def check(root: pathlib.Path, build: bool = False) -> tuple[list[str], int, int]:
    """Returns (errors, seeded_count, clean_count). Caller decides the exit code."""
    errors: list[str] = []
    man_path = root / "manifest.yaml"
    man = yaml.safe_load(man_path.read_text())
    items = man.get("items") if isinstance(man, dict) else None
    if not isinstance(items, list) or not items:
        return (["manifest 'items' list missing or empty"], 0, 0)

    ids: set[str] = set()
    seeded_n = clean_n = 0
    for it in items:
        iid = str(it.get("id"))
        if iid in ids:
            errors.append(f"duplicate id {iid!r}")
        ids.add(iid)
        d = root / "items" / iid
        ctx, dp, tp = d / "context.md", d / "diff.patch", d / "truth.yaml"
        fixture = d / "fixture"
        for f in (ctx, dp, tp):
            if not f.is_file():
                errors.append(f"{iid}: missing {f.name}")
        if not fixture.is_dir():
            errors.append(f"{iid}: missing fixture/ dir")
        if not tp.is_file():
            continue
        truth = yaml.safe_load(tp.read_text()) or {}
        seeded = bool(truth.get("seeded"))
        if seeded != bool(it.get("seeded")):
            errors.append(f"{iid}: manifest seeded={it.get('seeded')} != truth {truth.get('seeded')}")
        if ctx.is_file() and len(ctx.read_text().splitlines()) > 25:
            errors.append(f"{iid}: context.md > 25 lines")
        if dp.is_file():
            t = dp.read_text()
            if "--- " not in t or "+++ " not in t or "@@" not in t:
                errors.append(f"{iid}: diff.patch missing ---/+++/@@ markers")

        defects = truth.get("defects") or []
        # neutral_findings (OPTIONAL, seeded-only): true-but-out-of-scope
        # observations the judge treats as neither credit nor false finding.
        # Clean items stay strict -- they may NOT carry any neutral_findings.
        neutral = truth.get("neutral_findings")
        if seeded:
            seeded_n += 1
            if not (1 <= len(defects) <= 2):
                errors.append(f"{iid}: seeded needs 1-2 defects, has {len(defects)}")
            did: set = set()
            for j, df in enumerate(defects):
                if not isinstance(df, dict):
                    errors.append(f"{iid} defect[{j}]: not a mapping")
                    continue
                for k in DEFECT_FIELDS:
                    if df.get(k) in (None, "", []):
                        errors.append(f"{iid} defect[{j}]: missing/empty '{k}'")
                if df.get("id") in did:
                    errors.append(f"{iid}: duplicate defect id {df.get('id')!r}")
                did.add(df.get("id"))
            if neutral is not None:
                if not isinstance(neutral, list) or not neutral:
                    errors.append(f"{iid}: neutral_findings must be a non-empty list when present")
                else:
                    for j, nf in enumerate(neutral):
                        if not isinstance(nf, str) or not nf.strip():
                            errors.append(f"{iid} neutral_findings[{j}]: must be a non-empty string")
            if build and fixture.is_dir():
                _build_fixture(fixture, iid, errors)
        else:
            clean_n += 1
            if defects:
                errors.append(f"{iid}: clean item must have 0 defects, has {len(defects)}")
            if neutral is not None:
                errors.append(f"{iid}: clean item must not carry neutral_findings (seeded-only field)")
            if not (truth.get("clean_rationale") or "").strip():
                errors.append(f"{iid}: clean item missing clean_rationale")
            tnd = truth.get("tempting_non_defects")
            if not isinstance(tnd, list) or not tnd:
                errors.append(f"{iid}: clean item missing non-empty tempting_non_defects list")
            if build and fixture.is_dir():
                _build_fixture(fixture, iid, errors)

    idir = root / "items"
    if idir.is_dir():
        orphan = {p.name for p in idir.iterdir() if p.is_dir()} - ids
        if orphan:
            errors.append(f"item dirs not in manifest: {sorted(orphan)}")

    return errors, seeded_n, clean_n


def build_arg_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="check_taskset",
        description=(
            "Validate a review-seeded taskset dir: schema/field completeness, "
            "seeded/clean exclusivity, diff shape, fixture presence, no orphan dirs."
        ),
    )
    p.add_argument("taskset_dir", help="e.g. evals/tasksets/review-seeded-v1")
    p.add_argument(
        "--build",
        action="store_true",
        help=(
            "also `cargo build -j 1` every item's fixture/ (slow -- off by default; "
            "the M3 DoD runs this as a separate gate over the full corpus)"
        ),
    )
    return p


def main(argv=None) -> int:
    args = build_arg_parser().parse_args(argv)
    root = pathlib.Path(args.taskset_dir)
    if not root.is_dir():
        print(f"FAIL: taskset directory not found: {root}", file=sys.stderr)
        return 2
    man_path = root / "manifest.yaml"
    if not man_path.is_file():
        print(f"FAIL: missing {man_path}", file=sys.stderr)
        return 2

    errors, seeded_n, clean_n = check(root, build=args.build)
    if errors:
        print("\n".join(errors))
        print(f"FAIL: {len(errors)} error(s)", file=sys.stderr)
        return 1
    total = seeded_n + clean_n
    rate = seeded_n / total if total else 0.0
    print(f"OK: {total} items - {seeded_n} seeded / {clean_n} clean, base rate {rate:.2f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
