#!/usr/bin/env python3
"""Run the official SWE-bench harness for one concurrent pair.

The Rust runner captures this script's stdout as `harness-result.json`, so all
official harness output is redirected to sidecar logs and only the compact
stateful-bench result schema is printed on stdout.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


MODEL_NAME = "stateful-bench"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pair-json", required=True)
    parser.add_argument("--combined-patch", required=True)
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--python", required=True)
    parser.add_argument("--dataset-name", default="SWE-bench/SWE-bench_Verified")
    parser.add_argument("--split", default="test")
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--max-workers", default="2")
    parser.add_argument("--timeout", default="1800")
    parser.add_argument("--cache-level", default="env")
    parser.add_argument("--namespace", default="swebench")
    args = parser.parse_args()

    pair_path = Path(args.pair_json).resolve()
    patch_path = Path(args.combined_patch).resolve()
    work_dir = Path(args.work_dir).resolve()
    work_dir.mkdir(parents=True, exist_ok=True)

    pair = json.loads(pair_path.read_text())
    patch = patch_path.read_text()
    task_ids = [pair["task_a"]["instance_id"], pair["task_b"]["instance_id"]]

    predictions_path = work_dir / "predictions.jsonl"
    with predictions_path.open("w", encoding="utf-8") as handle:
        for instance_id in task_ids:
            print(
                json.dumps(
                    {
                        "instance_id": instance_id,
                        "model_name_or_path": MODEL_NAME,
                        "model_patch": patch,
                    }
                ),
                file=handle,
            )

    command = [
        args.python,
        "-m",
        "swebench.harness.run_evaluation",
        "--dataset_name",
        args.dataset_name,
        "--split",
        args.split,
        "--instance_ids",
        *task_ids,
        "--predictions_path",
        str(predictions_path),
        "--max_workers",
        args.max_workers,
        "--timeout",
        args.timeout,
        "--cache_level",
        args.cache_level,
        "--run_id",
        args.run_id,
        "--namespace",
        args.namespace,
    ]

    stdout_log = work_dir / "swebench.stdout.log"
    stderr_log = work_dir / "swebench.stderr.log"
    with stdout_log.open("w", encoding="utf-8") as stdout, stderr_log.open(
        "w", encoding="utf-8"
    ) as stderr:
        completed = subprocess.run(
            command,
            cwd=work_dir,
            stdout=stdout,
            stderr=stderr,
            check=False,
        )

    result = {
        "task_results": [],
        "metrics": {},
        "official_swebench": {
            "exit_code": completed.returncode,
            "run_id": args.run_id,
            "predictions_path": str(predictions_path),
            "stdout_log": str(stdout_log),
            "stderr_log": str(stderr_log),
        },
    }

    report_path = work_dir / f"{MODEL_NAME}.{args.run_id}.json"
    if completed.returncode != 0 or not report_path.is_file():
        result["task_results"] = [{"status": "setup_error"} for _ in task_ids]
        print(json.dumps(result))
        return 0

    report = json.loads(report_path.read_text())
    resolved = set(report.get("resolved_ids", []))
    unresolved = set(report.get("unresolved_ids", []))
    errors = set(report.get("error_ids", []))
    incomplete = set(report.get("incomplete_ids", []))
    result["official_swebench"]["report_path"] = str(report_path)
    result["official_swebench"]["report"] = report

    for instance_id in task_ids:
        if instance_id in resolved:
            status = "passed"
        elif instance_id in unresolved:
            status = "failed"
        elif instance_id in errors:
            status = "setup_error"
        elif instance_id in incomplete:
            status = "unknown"
        else:
            status = "unknown"
        result["task_results"].append(
            {
                "instance_id": instance_id,
                "status": status,
            }
        )

    print(json.dumps(result))
    return 0


if __name__ == "__main__":
    sys.exit(main())
