from __future__ import annotations

from argparse import Namespace

import pytest

from conftest import load_script


@pytest.fixture
def mod():
    return load_script("programbench_codex_agent.py")


def test_subagent_prompt_adds_workflowz_after_orchestrate(mod):
    off = mod.prompt_for_args(Namespace(benchmark_max_turns=500, subagent=False))
    on = mod.prompt_for_args(Namespace(benchmark_max_turns=500, subagent=True))

    assert "workflowz" not in off
    assert "workflowz" in on
    assert "\norchestrate\nworkflowz" in on
