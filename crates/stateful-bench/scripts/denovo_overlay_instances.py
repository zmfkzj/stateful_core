#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

from denovo_retry_overlay_report import (
    TrialSpec,
    collect_rows,
    parse_prefixed_values,
    row_key,
)


def score_value(row: dict[str, Any] | None) -> float | None:
    if row is None:
        return None
    score = row.get("score")
    if isinstance(score, (int, float)) and not isinstance(score, bool):
        return float(score)
    return None


def finish_reason(row: dict[str, Any] | None) -> str | None:
    if row is None:
        return None
    value = row.get("finish_reason")
    return str(value) if value is not None else "unknown"


def success_value(row: dict[str, Any] | None) -> bool | None:
    if row is None:
        return None
    value = row.get("success")
    return value if isinstance(value, bool) else None


def build_trial_specs(trial_args: list[str], retry_args: list[str]) -> list[TrialSpec]:
    trials = parse_prefixed_values(trial_args)
    retries = parse_prefixed_values(retry_args)
    specs: list[TrialSpec] = []
    for trial_id, base_prefixes in sorted(trials.items()):
        specs.append(TrialSpec(trial_id, base_prefixes, retries.get(trial_id, [])))
    for retry_trial in sorted(set(retries) - set(trials)):
        raise SystemExit(f"retry specified for unknown trial {retry_trial!r}")
    return specs


def effective_rows(
    runs_root: Path,
    trials: list[TrialSpec],
    retry_finish_reason: str,
) -> list[dict[str, Any]]:
    retry_rows_by_key: dict[tuple[str, str, str], dict[str, Any]] = {}
    for trial in trials:
        for item in collect_rows(runs_root, trial.retry_prefixes, trial.trial_id):
            retry_rows_by_key[row_key(trial.trial_id, item["condition_id"], item["row"])] = item[
                "row"
            ]

    rows: list[dict[str, Any]] = []
    for trial in trials:
        for item in collect_rows(runs_root, trial.base_prefixes, trial.trial_id):
            key = row_key(trial.trial_id, item["condition_id"], item["row"])
            retry_row = retry_rows_by_key.get(key)
            replaced = (
                item["row"].get("finish_reason") == retry_finish_reason
                and retry_row is not None
            )
            rows.append(
                {
                    "trial_id": trial.trial_id,
                    "condition_id": item["condition_id"],
                    "instance_id": key[2],
                    "row": retry_row if replaced else item["row"],
                    "replaced": replaced,
                }
            )
    return rows


def collect_instance_summary(
    runs_root: Path,
    trials: list[TrialSpec],
    retry_finish_reason: str = "codex-error",
) -> dict[str, Any]:
    by_pair: dict[tuple[str, str], dict[str, dict[str, Any]]] = {}
    for item in effective_rows(runs_root, trials, retry_finish_reason):
        pair = (item["trial_id"], item["instance_id"])
        by_pair.setdefault(pair, {})[item["condition_id"]] = item

    records: list[dict[str, Any]] = []
    for (trial_id, instance_id), conditions in sorted(by_pair.items()):
        off = conditions.get("stateful-off_subagent-on")
        on = conditions.get("stateful-on_subagent-on")
        off_row = off["row"] if off else None
        on_row = on["row"] if on else None
        off_score = score_value(off_row)
        on_score = score_value(on_row)
        delta = (
            on_score - off_score
            if off_score is not None and on_score is not None
            else None
        )
        records.append(
            {
                "trial_id": trial_id,
                "instance_id": instance_id,
                "off_score": off_score,
                "on_score": on_score,
                "score_delta_on_minus_off": delta,
                "off_finish_reason": finish_reason(off_row),
                "on_finish_reason": finish_reason(on_row),
                "off_success": success_value(off_row),
                "on_success": success_value(on_row),
                "off_replaced": bool(off and off["replaced"]),
                "on_replaced": bool(on and on["replaced"]),
            }
        )

    return {
        "records": records,
        "negative_scored_deltas": sorted(
            [
                record
                for record in records
                if record["score_delta_on_minus_off"] is not None
                and record["score_delta_on_minus_off"] < 0
            ],
            key=lambda record: record["score_delta_on_minus_off"],
        ),
        "on_runtime_failures": [
            record
            for record in records
            if record["on_finish_reason"] not in (None, "stop")
        ],
        "off_runtime_failures": [
            record
            for record in records
            if record["off_finish_reason"] not in (None, "stop")
        ],
    }


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="List DeNovoSWE instance-level off/on outcomes after retry overlay."
    )
    parser.add_argument("--runs-root", default="target/stateful-bench/denovo/runs")
    parser.add_argument("--trial", action="append", required=True)
    parser.add_argument("--retry", action="append", default=[])
    parser.add_argument("--retry-finish-reason", default="codex-error")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    summary = collect_instance_summary(
        Path(args.runs_root),
        build_trial_specs(args.trial, args.retry),
        args.retry_finish_reason,
    )
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
