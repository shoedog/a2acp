"""Config-guard tests -- pure logic, no model calls, no filesystem paths
beyond `tmp_path`. Covers the same-family judge guard and the count-based
budget gate, both of which must fire BEFORE a single agent turn is spent."""
from __future__ import annotations

import pytest

from harness import config as config_mod
from harness.config import Cell


def test_cross_family_judge_passes_for_all_three_cells():
    cells = list(config_mod.CELLS.values())
    # kiro (amazon) is cross-family from every cell's authorship set
    # (openai/anthropic) -- must not raise.
    config_mod.validate_same_family_judge(cells, "kiro")


def test_same_family_judge_is_refused():
    cells = [Cell("codex-solo", "code-review-codex-solo", ("codex",))]
    with pytest.raises(config_mod.ConfigError, match="shares a model family"):
        config_mod.validate_same_family_judge(cells, "codex")


def test_same_family_judge_override_is_honored():
    cells = [Cell("codex-solo", "code-review-codex-solo", ("codex",))]
    config_mod.validate_same_family_judge(cells, "codex", allow=True)


def test_unknown_judge_agent_is_a_config_error():
    with pytest.raises(config_mod.ConfigError):
        config_mod.validate_same_family_judge([], "nonexistent-agent")


def test_project_turn_count_counts_judge_calls_by_default():
    cells = [config_mod.CELLS["duo"], config_mod.CELLS["codex-solo"]]
    assert config_mod.project_turn_count(cells, n_items=15, judge=True) == 2 * 2 * 15
    assert config_mod.project_turn_count(cells, n_items=15, judge=False) == 2 * 15


def test_budget_gate_raises_when_over_cap():
    with pytest.raises(config_mod.BudgetExceeded):
        config_mod.budget_gate(projected=100, cap=99)


def test_budget_gate_allows_at_or_under_cap():
    config_mod.budget_gate(projected=99, cap=99)
    config_mod.budget_gate(projected=1, cap=99)


def test_load_manifest_missing_dir_is_config_error(tmp_path):
    with pytest.raises(config_mod.ConfigError, match="taskset directory not found"):
        config_mod.load_manifest(tmp_path / "does-not-exist")


def test_load_manifest_missing_manifest_yaml_is_config_error(tmp_path):
    (tmp_path / "items").mkdir()
    with pytest.raises(config_mod.ConfigError, match="manifest not found"):
        config_mod.load_manifest(tmp_path)


def test_load_manifest_missing_item_path_is_config_error(tmp_path):
    (tmp_path / "manifest.yaml").write_text("items:\n  - id: s1\n    seeded: true\n")
    item_dir = tmp_path / "items" / "s1"
    item_dir.mkdir(parents=True)
    (item_dir / "context.md").write_text("ctx")
    (item_dir / "diff.patch").write_text("--- a\n+++ b\n@@\n")
    # truth.yaml deliberately missing
    with pytest.raises(config_mod.ConfigError, match="missing"):
        config_mod.load_manifest(tmp_path)
