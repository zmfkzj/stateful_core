#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from collections import Counter, defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable


@dataclass(frozen=True)
class TrialSpec:
    trial_id: str
    base_prefixes: list[str]
    retry_prefixes: list[str]


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    if not path.exists():
        return rows
    with path.open("r", encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, start=1):
            stripped = line.strip()
            if not stripped:
                continue
            try:
                row = json.loads(stripped)
            except json.JSONDecodeError as error:
                raise ValueError(f"{path}:{line_number}: invalid JSONL row: {error}") from error
            if not isinstance(row, dict):
                raise ValueError(f"{path}:{line_number}: expected JSON object")
            rows.append(row)
    return rows


def run_dirs_for_prefixes(runs_root: Path, prefixes: Iterable[str]) -> list[Path]:
    dirs: set[Path] = set()
    for prefix in prefixes:
        dirs.update(path for path in runs_root.glob(f"{prefix}*") if path.is_dir())
    return sorted(dirs)


def result_files_for_run(run_dir: Path) -> list[Path]:
    return sorted((run_dir / "conditions").glob("*/*/_/results.jsonl"))


def condition_id_for_result_file(path: Path) -> str:
    return path.parents[2].name


def row_key(trial_id: str, condition_id: str, row: dict[str, Any]) -> tuple[str, str, str]:
    instance_id = row.get("instance_id")
    if not isinstance(instance_id, str) or not instance_id:
        raise ValueError(f"row in {trial_id}/{condition_id} is missing instance_id")
    return (trial_id, condition_id, instance_id)


def collect_rows(
    runs_root: Path,
    prefixes: Iterable[str],
    trial_id: str,
) -> list[dict[str, Any]]:
    collected: list[dict[str, Any]] = []
    for run_dir in run_dirs_for_prefixes(runs_root, prefixes):
        for result_path in result_files_for_run(run_dir):
            condition_id = condition_id_for_result_file(result_path)
            for row in read_jsonl(result_path):
                collected.append(
                    {
                        "trial_id": trial_id,
                        "run_id": run_dir.name,
                        "condition_id": condition_id,
                        "row": row,
                    }
                )
    return collected


def row_subagent_used(row: dict[str, Any]) -> bool | None:
    value = row.get("subagent_used")
    if isinstance(value, bool):
        return value
    usage = row.get("subagent_usage")
    if isinstance(usage, dict):
        nested = usage.get("subagent_used")
        if isinstance(nested, bool):
            return nested
    return None


def empty_stats() -> dict[str, Any]:
    return {
        "rows": 0,
        "success_count": 0,
        "score_sum": 0.0,
        "scored_count": 0,
        "finish_reasons": Counter(),
        "subagent_observed": 0,
        "subagent_used_count": 0,
        "replacement_count": 0,
    }


def add_row(stats: dict[str, Any], row: dict[str, Any], replaced: bool) -> None:
    stats["rows"] += 1
    if row.get("success") is True:
        stats["success_count"] += 1

    score = row.get("score")
    if isinstance(score, (int, float)) and not isinstance(score, bool):
        stats["score_sum"] += float(score)
        stats["scored_count"] += 1

    stats["finish_reasons"][str(row.get("finish_reason") or "unknown")] += 1

    subagent_used = row_subagent_used(row)
    if subagent_used is not None:
        stats["subagent_observed"] += 1
        if subagent_used:
            stats["subagent_used_count"] += 1

    if replaced:
        stats["replacement_count"] += 1


def finalized_stats(
    condition_id: str,
    stats: dict[str, Any],
    expected_instances_per_condition: int | None,
    expected_trial_count: int = 1,
) -> dict[str, Any]:
    rows = stats["rows"]
    scored_count = stats["scored_count"]
    subagent_observed = stats["subagent_observed"]
    expected_rows = (
        expected_instances_per_condition * expected_trial_count
        if expected_instances_per_condition
        else None
    )
    return {
        "condition_id": condition_id,
        "rows": rows,
        "success_count": stats["success_count"],
        "success_rate": stats["success_count"] / rows if rows else None,
        "average_score": stats["score_sum"] / scored_count if scored_count else None,
        "scored_count": scored_count,
        "finish_reasons": dict(sorted(stats["finish_reasons"].items())),
        "subagent_observed": subagent_observed,
        "subagent_used_count": stats["subagent_used_count"],
        "subagent_used_rate": (
            stats["subagent_used_count"] / subagent_observed if subagent_observed else None
        ),
        "replacement_count": stats["replacement_count"],
        "progress_rate": (
            rows / expected_rows
            if expected_rows
            else None
        ),
    }


def collect_overlay_summary(
    *,
    runs_root: Path,
    trials: list[TrialSpec],
    expected_instances_per_condition: int | None = None,
    retry_finish_reason: str = "codex-error",
) -> dict[str, Any]:
    condition_stats: dict[str, dict[str, Any]] = defaultdict(empty_stats)
    trial_stats: dict[tuple[str, str], dict[str, Any]] = defaultdict(empty_stats)
    total_base_rows = 0
    total_replacements = 0
    used_retry_keys: set[tuple[str, str, str]] = set()
    retry_rows_by_key: dict[tuple[str, str, str], dict[str, Any]] = {}
    retry_row_count = 0

    retry_rows_by_trial: dict[str, list[dict[str, Any]]] = {}
    for trial in trials:
        retry_rows = collect_rows(runs_root, trial.retry_prefixes, trial.trial_id)
        retry_rows_by_trial[trial.trial_id] = retry_rows
        retry_row_count += len(retry_rows)
        for item in retry_rows:
            key = row_key(trial.trial_id, item["condition_id"], item["row"])
            retry_rows_by_key[key] = item["row"]

    for trial in trials:
        for item in collect_rows(runs_root, trial.base_prefixes, trial.trial_id):
            row = item["row"]
            condition_id = item["condition_id"]
            key = row_key(trial.trial_id, condition_id, row)
            replacement = retry_rows_by_key.get(key)
            replaced = row.get("finish_reason") == retry_finish_reason and replacement is not None
            effective_row = replacement if replaced else row
            if replaced:
                total_replacements += 1
                used_retry_keys.add(key)
            total_base_rows += 1
            add_row(condition_stats[condition_id], effective_row, replaced)
            add_row(trial_stats[(trial.trial_id, condition_id)], effective_row, replaced)

    condition_summaries = [
        finalized_stats(
            condition_id,
            stats,
            expected_instances_per_condition,
            expected_trial_count=len(trials),
        )
        for condition_id, stats in sorted(condition_stats.items())
    ]
    trial_summaries = []
    for (trial_id, condition_id), stats in sorted(trial_stats.items()):
        summary = finalized_stats(condition_id, stats, expected_instances_per_condition)
        summary["trial_id"] = trial_id
        trial_summaries.append(summary)

    return {
        "trial_count": len(trials),
        "trials": trial_summaries,
        "conditions": condition_summaries,
        "expected_instances_per_condition": expected_instances_per_condition,
        "total_base_rows": total_base_rows,
        "total_effective_rows": sum(item["rows"] for item in condition_summaries),
        "total_replacements": total_replacements,
        "retry_row_count": retry_row_count,
        "unused_retry_rows": retry_row_count - len(used_retry_keys),
        "retry_finish_reason": retry_finish_reason,
    }


def parse_prefixed_values(values: list[str]) -> dict[str, list[str]]:
    parsed: dict[str, list[str]] = {}
    for value in values:
        if "=" not in value:
            raise SystemExit(f"expected NAME=PREFIX[,PREFIX...] but got {value!r}")
        name, prefixes = value.split("=", 1)
        name = name.strip()
        if not name:
            raise SystemExit(f"trial name is empty in {value!r}")
        parsed[name] = [prefix.strip() for prefix in prefixes.split(",") if prefix.strip()]
    return parsed


def build_trial_specs(trial_args: list[str], retry_args: list[str]) -> list[TrialSpec]:
    trials = parse_prefixed_values(trial_args)
    retries = parse_prefixed_values(retry_args)
    specs: list[TrialSpec] = []
    for trial_id, base_prefixes in sorted(trials.items()):
        specs.append(TrialSpec(trial_id, base_prefixes, retries.get(trial_id, [])))
    for retry_trial in sorted(set(retries) - set(trials)):
        raise SystemExit(f"retry specified for unknown trial {retry_trial!r}")
    return specs


def format_decimal(value: Any) -> str:
    if value is None:
        return "-"
    return f"{float(value):.3f}"


def render_markdown(summary: dict[str, Any]) -> str:
    lines = [
        "# DeNovoSWE Retry Overlay Report",
        "",
        f"Trials: {summary['trial_count']}",
        f"Base rows: {summary['total_base_rows']}",
        f"Effective rows: {summary['total_effective_rows']}",
        f"Retry replacements: {summary['total_replacements']}",
        f"Unused retry rows: {summary['unused_retry_rows']}",
        "",
        "| Condition | Rows | Success | Avg score | Scored | Finish reasons | Replacements |",
        "| --- | ---: | ---: | ---: | ---: | --- | ---: |",
    ]
    for condition in summary["conditions"]:
        finish_reasons = ", ".join(
            f"{reason}={count}"
            for reason, count in sorted(condition["finish_reasons"].items())
        ) or "-"
        lines.append(
            "| {condition_id} | {rows} | {success_count} ({success_rate}) | {average_score} | {scored_count} | {finish_reasons} | {replacement_count} |".format(
                condition_id=condition["condition_id"],
                rows=condition["rows"],
                success_count=condition["success_count"],
                success_rate=format_decimal(condition["success_rate"]),
                average_score=format_decimal(condition["average_score"]),
                scored_count=condition["scored_count"],
                finish_reasons=finish_reasons,
                replacement_count=condition["replacement_count"],
            )
        )

    lines.extend(
        [
            "",
            "| Trial | Condition | Rows | Success | Avg score | Replacements |",
            "| --- | --- | ---: | ---: | ---: | ---: |",
        ]
    )
    for trial in summary["trials"]:
        lines.append(
            "| {trial_id} | {condition_id} | {rows} | {success_count} ({success_rate}) | {average_score} | {replacement_count} |".format(
                trial_id=trial["trial_id"],
                condition_id=trial["condition_id"],
                rows=trial["rows"],
                success_count=trial["success_count"],
                success_rate=format_decimal(trial["success_rate"]),
                average_score=format_decimal(trial["average_score"]),
                replacement_count=trial["replacement_count"],
            )
        )
    return "\n".join(lines) + "\n"


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Aggregate DeNovoSWE runs while overlaying retry rows for codex-error failures."
    )
    parser.add_argument("--runs-root", default="target/stateful-bench/denovo/runs")
    parser.add_argument(
        "--trial",
        action="append",
        required=True,
        help="Trial base run prefixes as NAME=PREFIX[,PREFIX...].",
    )
    parser.add_argument(
        "--retry",
        action="append",
        default=[],
        help="Retry run prefixes as NAME=PREFIX[,PREFIX...].",
    )
    parser.add_argument("--retry-finish-reason", default="codex-error")
    parser.add_argument("--expected-instances-per-condition", type=int)
    parser.add_argument("--format", choices=["markdown", "json"], default="markdown")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    summary = collect_overlay_summary(
        runs_root=Path(args.runs_root),
        trials=build_trial_specs(args.trial, args.retry),
        expected_instances_per_condition=args.expected_instances_per_condition,
        retry_finish_reason=args.retry_finish_reason,
    )
    if args.format == "json":
        print(json.dumps(summary, indent=2, sort_keys=True))
    else:
        print(render_markdown(summary), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
