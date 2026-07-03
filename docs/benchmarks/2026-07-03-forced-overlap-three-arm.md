# Forced-overlap three-arm evidence — 2026-07-03

Evidence kind: `paired_agent_run`.
Empirical claim allowed by the benchmark reporter: yes.
Trials: 3.
Manifest: 15 exact-file-overlap pairs generated with seed 42.
Model: default OMP model used by `overlap_omp_agent.py` for all arms.
Arms: no-state, awareness, stateful/enforcement.
Evaluator: checked-in `crates/stateful-bench/scripts/overlap_harness.py` through `stateful-bench run` + `stateful-bench compare`.
Raw logs: intentionally not checked in.
Checked artifacts: `compare-t1.md/json`, `compare-t2.md/json`, `compare-t3.md/json`.

## Aggregate result

| Metric | No-state mean ± σ | Awareness mean ± σ | Stateful mean ± σ |
| --- | ---: | ---: | ---: |
| Available valid functional score | 0.144 ± 0.000 | 0.144 ± 0.000 | 0.144 ± 0.000 |
| Raw manifest functional score | 0.144 ± 0.000 | 0.144 ± 0.000 | 0.144 ± 0.000 |
| Scored pairs | 15 ± 0 | 15 ± 0 | 15 ± 0 |
| Preserved edit count | 5 ± 0 | 5 ± 0 | 5 ± 0 |
| Missing expected line count | 27 ± 0 | 27 ± 0 | 27 ± 0 |
| Uncoordinated same-file collisions | 0 ± 0 | 0 ± 0 | 0 ± 0 |
| Lost edit events | 0 ± 0 | 0 ± 0 | 0 ± 0 |
| Coordinated blocks | 0 ± 0 | 0 ± 0 | 0 ± 0 |
| Denied writes | 0 ± 0 | 0 ± 0 | 0 ± 0 |
| False blocks | 0 ± 0 | 0 ± 0 | 0 ± 0 |
| Manual interventions | 0 ± 0 | 0 ± 0 | 0 ± 0 |
| Wall time ms | 55,271 ± 120 | 54,449 ± 178 | 54,257 ± 291 |

## Pairwise wall-time deltas

Negative means the right-hand coordinated arm was faster in the recorded run.

| Comparison | Trial deltas ms | Mean ± σ ms |
| --- | ---: | ---: |
| no-state → awareness | -1085, -907, -474 | -822 ± 257 |
| awareness → stateful | -392, -139, -45 | -192 ± 147 |
| no-state → stateful | -1477, -1046, -519 | -1014 ± 392 |

## Verdict against ADR-0002 hypothesis

The forced-overlap harness did not produce differentiated safety outcomes in these three trials: all three arms had the same functional score, preserved edit count, and missing expected line count. That means this evidence is a plumbing/smoke result for the three-arm runner and compare path, not proof that awareness preserves stateful safety at lower coordination cost.

ADR-0002 should remain evidence-gated: no reservation/blocking demotion follows from this run alone.

## Omitted / non-comparable metrics

- Duplicated investigation time: not measured by this harness.
- Human intervention count: no human-in-the-loop path in this harness; reported as 0 when no harness event exists.
- Time to converge: not emitted by the overlap harness; per-trial compare reports show `n/a`.
- Token and tool-call counts: not emitted by the overlap harness for these runs.

## Scrub check

Before check-in, run:

```bash
grep -R "authorization-token-marker\|<local-home>" docs/benchmarks
```

Expected result: no matches.
