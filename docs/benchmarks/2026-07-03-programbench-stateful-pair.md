# ProgramBench stateful pair evidence — 2026-07-03

Evidence kind: `programbench_agent_run` for inference artifacts; official quality scoring is incomplete.
Trials: 3 inference trials.
Conditions: `stateful:off,subagent:on` and `stateful:on,subagent:on`.
Agent: `omp-cli`.
Model: `openai-codex/gpt-5.4-mini` with minimal thinking.
Instance set: first three ProgramBench tasks by installed dataset order after the requested `ripgrep.*` filter yielded no instances:

1. `abishekvashok__cmatrix.5c082c6`
2. `agourlay__zip-password-finder.704700d`
3. `ajeetdsouza__zoxide.67ca1bc`

ProgramBench install used the upstream GitHub package on Python 3.11 because the local Python 3.9 install had only the placeholder PyPI package.

## Inference run summary

| Trial | Condition | Exit codes | Wall time ms | Turns | Input+output tokens | Uncached input+output tokens |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| 1 | stateful off + subagent on | 0, 0, 0 | 585,962 | 167 | 424,282 | 327,187 |
| 1 | stateful on + subagent on | 0, 0, 0 | 650,508 | 243 | 442,356 | 277,229 |
| 2 | stateful off + subagent on | 0, 0, 0 | 532,759 | 224 | 406,984 | 254,485 |
| 2 | stateful on + subagent on | 0, 0, 0 | 867,938 | 265 | 349,179 | 141,828 |
| 3 | stateful off + subagent on | 1, 0, 0 | 533,285 | 205 | 351,416 | 179,858 |
| 3 | stateful on + subagent on | 0, 0, 0 | 941,370 | 416 | 429,318 | 167,235 |

| Condition | Wall time mean ± σ ms | Turns mean ± σ | Input+output tokens mean ± σ | Uncached input+output tokens mean ± σ |
| --- | ---: | ---: | ---: | ---: |
| stateful off + subagent on | 550,669 ± 24,957 | 199 ± 24 | 394,227 ± 31,085 | 253,843 ± 60,149 |
| stateful on + subagent on | 819,939 ± 123,499 | 308 ± 77 | 406,951 ± 41,196 | 195,431 ± 58,763 |

## Official eval status

Official ProgramBench scoring did not finish on this macOS arm64 Docker host within the 3600s tool limit.

Observed attempts:

- `stateful-bench programbench eval --run-dir .../pb-pair-t1 --workers 4 --branch-workers 2 --docker-cpus 8` timed out after 3600s at 2/3 evaluated instances for the first condition.
- Re-running through the Python eval harness with a requested longer timeout hit the same 3600s execution ceiling and produced the same two eval artifacts.
- The orphaned Docker containers from both interrupted eval attempts were removed.

Partial official scores available for `pb-pair-t1/stateful-off_subagent-on` only:

| Instance | Score | Tests |
| --- | ---: | ---: |
| `agourlay__zip-password-finder.704700d` | 17 | 680 |
| `ajeetdsouza__zoxide.67ca1bc` | 9 | 531 |
| Average over evaluated instances | 13 | 2 instances |

No official score is reported for `abishekvashok__cmatrix.5c082c6` or for the stateful-on condition. Do not use this file for quality comparisons.

## Run caveats

- Trial 3 `stateful-off_subagent-on` failed smoke compile for `abishekvashok__cmatrix.5c082c6` because the generated `cmatrix.c` referenced `useconds_t` without a visible declaration.
- The inference runs are useful for lifecycle, wall-clock, and token plumbing only.
- A complete ProgramBench quality comparison still requires rerunning official eval on a Linux amd64-compatible host, preserving the same model, instance set, Docker image, and three trials.

## Scrub check

Before check-in, run:

```bash
grep -R "authorization-token-marker\|<local-home>" docs/benchmarks
```

Expected result: no matches.
