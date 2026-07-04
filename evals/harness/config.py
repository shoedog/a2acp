"""Fail-fast experiment config for the M3 eval harness.

Ported IN SPIRIT from prompts-skills-steering's `harness/config.py`: validate
every referenced path against disk BEFORE spending a single agent turn, and
refuse a same-family judge before spending anything. Rebuilt around the
bridge's own vocabulary rather than steering's promptfoo-era one: there is no
baseline/treatment ARM split and no separate `ExecutorCfg`/`JudgeCfg` pair --
a CELL is just a {workflow_id, authorship_agents} tuple driven straight
through `a2a-bridge run-workflow`, and the blind judge is itself just another
workflow (a single kiro node) run the exact same way.

See docs/superpowers/specs/2026-07-04-m3-eval-harness.md for the spec this
implements (v2.1 -- gemini's OAuth is dead on this machine, judge pivoted to
kiro).
"""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

import yaml

HARNESS_DIR = Path(__file__).resolve().parent
EVALS_ROOT = HARNESS_DIR.parent
REPO_ROOT = EVALS_ROOT.parent

DEFAULT_BRIDGE_CONFIG = EVALS_ROOT / "configs" / "eval-agents.toml"
DEFAULT_TASKSET_DIR = EVALS_ROOT / "tasksets" / "review-seeded-v1"
DEFAULT_RUBRIC_PATH = EVALS_ROOT / "rubrics" / "review_judge.md"
DEFAULT_RESULTS_ROOT = EVALS_ROOT / "results"
DEFAULT_BRIDGE_BIN = REPO_ROOT / "target" / "release" / "a2a-bridge"


class ConfigError(Exception):
    """Raised when the harness config, or a path it references, is invalid."""


class BudgetExceeded(Exception):
    """Raised by `budget_gate` when the projected turn count exceeds the cap."""


# --------------------------------------------------------------------------- #
# Cells + the cross-family judge guard.
# --------------------------------------------------------------------------- #
# Harness-side agent-id -> model-family map. `AgentEntry` (bridge-core) has
# no family field, so the self-preference guard below has to own its own
# mapping rather than reading one out of the bridge's config -- exactly the
# situation the M3 spec's "config.py owns a harness-side family map" line
# describes. Keyed by the AGENT ID as used in evals/configs/eval-agents.toml
# (not by `cmd`), since that's the granularity `Cell.authorship_agents` and
# `JUDGE_AGENT_ID` operate at.
AGENT_FAMILY: dict[str, str] = {
    "codex": "openai",
    "claude": "anthropic",
    "gemini": "google",  # not wired into eval-agents.toml (dead OAuth, v2.1) -- kept for the map's completeness / a future re-entry.
    "kiro": "amazon",
}


@dataclass(frozen=True)
class Cell:
    id: str
    workflow_id: str
    # Every agent id that authors a byte of the GRADED artifact for this cell
    # -- reviewers AND, where one exists, the synth pass -- not just "the
    # executor". `duo`'s synth is claude, same agent (and so same family) as
    # the architecture reviewer: the tuple below enumerates it anyway
    # (duplicate on purpose) so the authorship set is auditable against the
    # spec text ("duo = codex+claude+claude-synth") even though family-set
    # dedup makes the duplicate a no-op for the guard itself.
    authorship_agents: tuple[str, ...]


CELLS: dict[str, Cell] = {
    "duo": Cell("duo", "code-review-duo", ("codex", "claude", "claude")),
    "codex-solo": Cell("codex-solo", "code-review-codex-solo", ("codex",)),
    "claude-solo": Cell("claude-solo", "code-review-claude-solo", ("claude",)),
}

JUDGE_AGENT_ID = "kiro"
JUDGE_WORKFLOW_ID = "judge"


def authorship_families(cell: Cell) -> set[str]:
    fams: set[str] = set()
    for agent_id in cell.authorship_agents:
        fam = AGENT_FAMILY.get(agent_id)
        if fam is None:
            raise ConfigError(
                f"cell {cell.id!r}: authorship agent {agent_id!r} has no entry in "
                f"AGENT_FAMILY -- add it before using this agent in a graded cell"
            )
        fams.add(fam)
    return fams


def validate_same_family_judge(
    cells: list[Cell], judge_agent_id: str, allow: bool = False
) -> None:
    """Self-preference guard: refuse a judge whose model family appears
    anywhere in the graded-artifact authorship set of ANY cell about to run,
    unless explicitly overridden.

    Generalized from steering's `_validate_same_family_judge` (a single fixed
    `executor.provider == judge.provider == "codex"` check) to an arbitrary
    per-cell authorship SET, because a `duo` cell's graded artifact has three
    authoring turns (two independent reviewers + a synth), not one executor
    -- the M3 spec is explicit that "the guard covers the whole authorship
    set, not just executors".
    """
    judge_family = AGENT_FAMILY.get(judge_agent_id)
    if judge_family is None:
        raise ConfigError(
            f"judge agent {judge_agent_id!r} has no entry in AGENT_FAMILY -- "
            f"add it before using this agent as the judge"
        )
    if allow:
        return
    for cell in cells:
        fams = authorship_families(cell)
        if judge_family in fams:
            raise ConfigError(
                f"judge agent {judge_agent_id!r} (family {judge_family!r}) shares a "
                f"model family with cell {cell.id!r}'s graded-artifact authorship set "
                f"{sorted(fams)} (agents {cell.authorship_agents!r}) -- a same-family "
                f"judge risks self-preference bias (the judge favoring outputs from "
                f"its own model family), undermining the point of a cross-family "
                f"grade. Pass --allow-same-family-judge to override if this is "
                f"intentional."
            )


# --------------------------------------------------------------------------- #
# Taskset loading -- fail fast: every referenced path is asserted BEFORE the
# caller spends a single agent turn on any of it.
# --------------------------------------------------------------------------- #
@dataclass
class ManifestItem:
    id: str
    seeded: bool
    item_dir: Path
    context_path: Path
    diff_path: Path
    truth_path: Path
    fixture_dir: Path


def load_manifest(taskset_dir: Path | str, limit: int | None = None) -> list[ManifestItem]:
    """Load a taskset's manifest.yaml + per-item paths.

    Every item's context.md/diff.patch/truth.yaml/fixture/ is asserted to
    exist here -- this is the "fail fast" half of the ported config.py
    pattern (steering's `load()` + `_validate_paths` did the equivalent for
    its baseline/varied-element artifact paths). Full SCHEMA validation
    (defect field completeness, seeded/clean exclusivity, etc.) is
    `check_taskset.py`'s job, not this loader's -- this only checks that the
    paths a run would need are actually there.
    """
    taskset_dir = Path(taskset_dir)
    if not taskset_dir.is_dir():
        raise ConfigError(f"taskset directory not found: {taskset_dir}")
    manifest_path = taskset_dir / "manifest.yaml"
    if not manifest_path.is_file():
        raise ConfigError(f"taskset manifest not found: {manifest_path}")
    manifest = yaml.safe_load(manifest_path.read_text()) or {}
    entries = manifest.get("items") or []
    if not isinstance(entries, list) or not entries:
        raise ConfigError(f"{manifest_path}: 'items' list missing or empty")

    selected = entries[: limit] if limit is not None else entries
    out: list[ManifestItem] = []
    for entry in selected:
        item_id = str(entry["id"])
        seeded = bool(entry.get("seeded", False))
        item_dir = taskset_dir / "items" / item_id
        context_path = item_dir / "context.md"
        diff_path = item_dir / "diff.patch"
        truth_path = item_dir / "truth.yaml"
        fixture_dir = item_dir / "fixture"
        for p in (context_path, diff_path, truth_path):
            if not p.is_file():
                raise ConfigError(f"item {item_id!r}: missing {p}")
        if not fixture_dir.is_dir():
            raise ConfigError(f"item {item_id!r}: missing fixture dir {fixture_dir}")
        out.append(
            ManifestItem(
                id=item_id,
                seeded=seeded,
                item_dir=item_dir,
                context_path=context_path,
                diff_path=diff_path,
                truth_path=truth_path,
                fixture_dir=fixture_dir,
            )
        )
    return out


# --------------------------------------------------------------------------- #
# Budget gate -- COUNT-based, not dollar-based.
# --------------------------------------------------------------------------- #
def project_turn_count(cells: list[Cell], n_items: int, judge: bool = True) -> int:
    """Projected number of `a2a-bridge run-workflow` invocations a full run
    would spend: one per (cell x item) -- a `duo` cell's 3 nodes are ONE
    `run-workflow` process from the harness's point of view, since the
    bridge fans the nodes out internally, not the harness -- plus, if
    `judge`, one additional judge invocation per graded (cell x item)."""
    per_item = len(cells) * (2 if judge else 1)
    return per_item * n_items


def budget_gate(projected: int, cap: int) -> None:
    """Abort BEFORE spending anything if the projected turn count exceeds
    `cap`.

    Generalizes steering's `run.py` two-arm cost gate
    (`baseline_cost * 2 > max_cost_usd`, computed from REAL spend after the
    baseline arm already ran) into a projection computed BEFORE any spend at
    all. This harness also has no live token/cost accounting available from
    `a2a-bridge run-workflow` (`NodeFinished.usage` is captured internally
    per Slice 10, but the CLI currently discards it -- `main.rs`'s
    `usage: _`), so this gate is a TURN COUNT, not a dollar figure -- see the
    M3 spec's "budget gate = projected run count vs cap".
    """
    if projected > cap:
        raise BudgetExceeded(
            f"projected {projected} turn(s) exceeds the configured cap ({cap}); "
            f"aborting before spending anything. Pass a higher --cap if this is "
            f"intentional."
        )


# --------------------------------------------------------------------------- #
# Path resolution helpers (also fail fast).
# --------------------------------------------------------------------------- #
def resolve_bridge_bin(path: Path | str | None) -> Path:
    p = Path(path) if path is not None else DEFAULT_BRIDGE_BIN
    if not p.is_file():
        raise ConfigError(
            f"a2a-bridge binary not found: {p} -- build it first "
            f"(`cargo build --release -p a2a-bridge` from the repo root) or pass "
            f"--bridge-bin explicitly."
        )
    return p


def resolve_bridge_config(path: Path | str | None) -> Path:
    p = Path(path) if path is not None else DEFAULT_BRIDGE_CONFIG
    if not p.is_file():
        raise ConfigError(f"bridge config not found: {p}")
    return p


def resolve_rubric(path: Path | str | None) -> Path:
    p = Path(path) if path is not None else DEFAULT_RUBRIC_PATH
    if not p.is_file():
        raise ConfigError(f"judge rubric not found: {p}")
    return p
