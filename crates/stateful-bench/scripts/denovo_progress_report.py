#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from collections import Counter
from pathlib import Path
from typing import Any, Iterable


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
                value = json.loads(stripped)
            except json.JSONDecodeError as error:
                raise ValueError(f"{path}:{line_number}: invalid JSONL row: {error}") from error
            if not isinstance(value, dict):
                raise ValueError(f"{path}:{line_number}: expected JSON object")
            rows.append(value)
    return rows


def read_json_object(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as handle:
        value = json.load(handle)
    if not isinstance(value, dict):
        raise ValueError(f"{path}: expected JSON object")
    return value


def report_files_for_run(run_dir: Path) -> list[Path]:
    conditions_dir = run_dir / "conditions"
    if not conditions_dir.exists():
        return []
    return sorted(conditions_dir.glob("*/denovo-report.json"))


def result_files_for_run(run_dir: Path) -> list[Path]:
    conditions_dir = run_dir / "conditions"
    if not conditions_dir.exists():
        return []
    return sorted(conditions_dir.glob("*/*/_/results.jsonl"))


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
        "setup_errors": 0,
        "finish_reasons": Counter(),
        "subagent_observed": 0,
        "subagent_used_count": 0,
    }


def add_row(stats: dict[str, Any], row: dict[str, Any]) -> None:
    stats["rows"] += 1
    if row.get("success") is True:
        stats["success_count"] += 1

    score = row.get("score")
    if isinstance(score, (int, float)) and not isinstance(score, bool):
        stats["score_sum"] += float(score)
        stats["scored_count"] += 1

    finish_reason = row.get("finish_reason") or "unknown"
    stats["finish_reasons"][str(finish_reason)] += 1
    if finish_reason == "setup-error":
        stats["setup_errors"] += 1

    subagent_used = row_subagent_used(row)
    if subagent_used is not None:
        stats["subagent_observed"] += 1
        if subagent_used:
            stats["subagent_used_count"] += 1


def add_summary(stats: dict[str, Any], summary: dict[str, Any]) -> None:
    rows = int(summary.get("rows") or 0)
    stats["rows"] += rows
    stats["success_count"] += int(summary.get("success_count") or 0)

    average_score = summary.get("average_score")
    scored_count = int(summary.get("scored_count") or 0)
    if isinstance(average_score, (int, float)) and not isinstance(average_score, bool):
        stats["score_sum"] += float(average_score) * scored_count
        stats["scored_count"] += scored_count

    stats["setup_errors"] += int(summary.get("setup_errors") or 0)
    stats["finish_reasons"].update(summary.get("finish_reasons") or {})
    stats["subagent_observed"] += int(summary.get("subagent_observed") or 0)
    stats["subagent_used_count"] += int(summary.get("subagent_used_count") or 0)


def finalized_stats(
    stats: dict[str, Any],
    expected_instances_per_condition: int | None,
) -> dict[str, Any]:
    rows = stats["rows"]
    scored_count = stats["scored_count"]
    subagent_observed = stats["subagent_observed"]
    return {
        "rows": rows,
        "success_count": stats["success_count"],
        "success_rate": stats["success_count"] / rows if rows else None,
        "average_score": stats["score_sum"] / scored_count if scored_count else None,
        "scored_count": scored_count,
        "setup_errors": stats["setup_errors"],
        "finish_reasons": dict(sorted(stats["finish_reasons"].items())),
        "subagent_observed": subagent_observed,
        "subagent_used_count": stats["subagent_used_count"],
        "subagent_used_rate": (
            stats["subagent_used_count"] / subagent_observed if subagent_observed else None
        ),
        "progress_rate": (
            rows / expected_instances_per_condition
            if expected_instances_per_condition
            else None
        ),
    }


def summarize_file(
    run_dir: Path,
    result_path: Path,
    expected_instances_per_condition: int | None,
) -> dict[str, Any]:
    rows = read_jsonl(result_path)
    condition_id = result_path.parents[2].name
    agent = result_path.parents[1].name
    stats = empty_stats()
    for row in rows:
        add_row(stats, row)
    summary = finalized_stats(stats, expected_instances_per_condition)
    summary.update(
        {
            "run_id": run_dir.name,
            "condition_id": condition_id,
            "agent": agent,
            "results_jsonl": str(result_path),
            "source": "results.jsonl",
        }
    )
    return summary


def numeric_or_none(value: Any) -> float | None:
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return float(value)
    return None


def int_or_zero(value: Any) -> int:
    if isinstance(value, bool):
        return 0
    if isinstance(value, int):
        return value
    if isinstance(value, float):
        return int(value)
    return 0


def summarize_report(
    run_dir: Path,
    report_path: Path,
    expected_instances_per_condition: int | None,
) -> dict[str, Any]:
    report = read_json_object(report_path)
    condition_id = str(report.get("condition_id") or report_path.parent.name)
    rows = int_or_zero(
        report.get("total_instances")
        if report.get("total_instances") is not None
        else report.get("completed_instances")
    )
    success_count = int_or_zero(report.get("success_count"))
    average_score = numeric_or_none(report.get("average_score"))
    scored_count = int_or_zero(report.get("scored_instances"))
    if average_score is not None and scored_count == 0 and rows:
        scored_count = rows

    subagent_observed = int_or_zero(report.get("subagent_observed_instances"))
    subagent_used_count = int_or_zero(report.get("subagent_used_count"))
    subagent_used_rate = numeric_or_none(report.get("subagent_used_rate"))
    if subagent_used_rate is None and subagent_observed:
        subagent_used_rate = subagent_used_count / subagent_observed

    return {
        "run_id": run_dir.name,
        "condition_id": condition_id,
        "agent": str(report.get("agent") or "condition-report"),
        "rows": rows,
        "success_count": success_count,
        "success_rate": success_count / rows if rows else None,
        "average_score": average_score,
        "scored_count": scored_count,
        "setup_errors": int_or_zero(report.get("setup_errors")),
        "finish_reasons": {},
        "subagent_observed": subagent_observed,
        "subagent_used_count": subagent_used_count,
        "subagent_used_rate": subagent_used_rate,
        "progress_rate": (
            rows / expected_instances_per_condition
            if expected_instances_per_condition
            else None
        ),
        "report_json": str(report_path),
        "source": "denovo-report.json",
    }


def collect_progress(
    run_dirs: Iterable[Path | str],
    *,
    expected_instances_per_condition: int | None = None,
) -> dict[str, Any]:
    normalized_run_dirs = [Path(run_dir) for run_dir in run_dirs]
    run_summaries: list[dict[str, Any]] = []
    condition_stats: dict[str, dict[str, Any]] = {}

    for run_dir in normalized_run_dirs:
        reported_conditions: set[str] = set()
        for report_path in report_files_for_run(run_dir):
            summary = summarize_report(run_dir, report_path, expected_instances_per_condition)
            run_summaries.append(summary)
            condition_id = summary["condition_id"]
            reported_conditions.add(condition_id)
            aggregate = condition_stats.setdefault(condition_id, empty_stats())
            add_summary(aggregate, summary)

        for result_path in result_files_for_run(run_dir):
            condition_id = result_path.parents[2].name
            if condition_id in reported_conditions:
                continue
            summary = summarize_file(run_dir, result_path, expected_instances_per_condition)
            run_summaries.append(summary)
            aggregate = condition_stats.setdefault(condition_id, empty_stats())
            add_summary(aggregate, summary)

    condition_summaries: list[dict[str, Any]] = []
    for condition_id, stats in sorted(condition_stats.items()):
        summary = finalized_stats(stats, expected_instances_per_condition)
        summary["condition_id"] = condition_id
        condition_summaries.append(summary)

    return {
        "run_count": len(normalized_run_dirs),
        "expected_instances_per_condition": expected_instances_per_condition,
        "total_result_rows": sum(run["rows"] for run in run_summaries),
        "conditions": condition_summaries,
        "runs": sorted(
            run_summaries,
            key=lambda run: (run["run_id"], run["condition_id"], run["agent"]),
        ),
    }


def format_decimal(value: Any) -> str:
    if value is None:
        return "-"
    return f"{float(value):.3f}"


def format_count_rate(count: int, total: int, rate: float | None) -> str:
    if rate is None:
        return f"{count}/{total}"
    return f"{count}/{total} ({rate:.3f})"


def format_finish_reasons(reasons: dict[str, int]) -> str:
    if not reasons:
        return "-"
    return ", ".join(f"{reason}={count}" for reason, count in sorted(reasons.items()))


def render_markdown(summary: dict[str, Any]) -> str:
    lines = [
        "# DeNovoSWE Progress",
        "",
        f"Run dirs: {summary['run_count']}",
        f"Total result rows: {summary['total_result_rows']}",
    ]
    expected = summary.get("expected_instances_per_condition")
    if expected:
        lines.append(f"Expected instances per condition: {expected}")
    lines.extend(
        [
            "",
            "| Condition | Rows | Progress | Success | Avg score | Setup errors | Finish reasons | Subagent |",
            "| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: |",
        ]
    )
    for condition in summary["conditions"]:
        rows = condition["rows"]
        progress = format_decimal(condition["progress_rate"])
        success = format_count_rate(condition["success_count"], rows, condition["success_rate"])
        subagent = format_count_rate(
            condition["subagent_used_count"],
            condition["subagent_observed"],
            condition["subagent_used_rate"],
        )
        lines.append(
            "| {condition_id} | {rows} | {progress} | {success} | {average_score} | {setup_errors} | {finish_reasons} | {subagent} |".format(
                condition_id=condition["condition_id"],
                rows=rows,
                progress=progress,
                success=success,
                average_score=format_decimal(condition["average_score"]),
                setup_errors=condition["setup_errors"],
                finish_reasons=format_finish_reasons(condition["finish_reasons"]),
                subagent=subagent,
            )
        )

    lines.extend(
        [
            "",
            "| Run | Condition | Agent | Rows | Success | Avg score | Setup errors | Finish reasons |",
            "| --- | --- | --- | ---: | ---: | ---: | ---: | --- |",
        ]
    )
    for run in summary["runs"]:
        rows = run["rows"]
        success = format_count_rate(run["success_count"], rows, run["success_rate"])
        lines.append(
            "| {run_id} | {condition_id} | {agent} | {rows} | {success} | {average_score} | {setup_errors} | {finish_reasons} |".format(
                run_id=run["run_id"],
                condition_id=run["condition_id"],
                agent=run["agent"],
                rows=rows,
                success=success,
                average_score=format_decimal(run["average_score"]),
                setup_errors=run["setup_errors"],
                finish_reasons=format_finish_reasons(run["finish_reasons"]),
            )
        )
    return "\n".join(lines) + "\n"


def discover_run_dirs(args: argparse.Namespace) -> list[Path]:
    if args.run_dir:
        return [Path(path) for path in args.run_dir]

    runs_root = Path(args.runs_root)
    prefixes = args.run_prefix or []
    if prefixes:
        run_dirs: list[Path] = []
        for prefix in prefixes:
            run_dirs.extend(sorted(runs_root.glob(f"{prefix}*")))
        return sorted({path for path in run_dirs if path.is_dir()})

    if runs_root.exists():
        return sorted(path for path in runs_root.glob("r*-denovo-shard-*") if path.is_dir())

    raise SystemExit("no run dirs provided and default runs root does not exist")


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Summarize in-progress DeNovoSWE benchmark shard results from results.jsonl files."
    )
    parser.add_argument("run_dir", nargs="*", help="Run directory such as target/.../r38-denovo-shard-a")
    parser.add_argument(
        "--runs-root",
        default="target/stateful-bench/denovo/runs",
        help="Root directory used with --run-prefix, or for default r*-denovo-shard-* discovery.",
    )
    parser.add_argument(
        "--run-prefix",
        action="append",
        help="Discover run directories under --runs-root whose names start with this prefix.",
    )
    parser.add_argument(
        "--expected-instances-per-condition",
        type=int,
        help="Expected completed rows for each condition; used only for progress percentages.",
    )
    parser.add_argument("--format", choices=["markdown", "json"], default="markdown")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    run_dirs = discover_run_dirs(args)
    summary = collect_progress(
        run_dirs,
        expected_instances_per_condition=args.expected_instances_per_condition,
    )
    if args.format == "json":
        print(json.dumps(summary, indent=2, sort_keys=True))
    else:
        print(render_markdown(summary), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
