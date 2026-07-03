# Stateful Bench Comparison

- Manifest pairs: 15
- Stateful run: overlap-stateful-t3
- No-state run: overlap-no-state-t3
- Evidence kind: paired_agent_run
- Empirical claim allowed: yes
- Evidence note: paired_agent_run evidence comes from executed agent pairs; effect-size claims still require an overlap-focused manifest, enough paired-valid samples, and overhead reporting.

## Paired Valid

| Metric | Value |
| --- | ---: |
| Paired valid pairs | 15 |
| Stateful functional score | 0.144 |
| No-state functional score | 0.144 |
| Paired valid functional delta | 0.000 |
| Raw manifest functional delta | 0.000 |

## Coordination Effects

| Metric | Delta |
| --- | ---: |
| Prevented uncoordinated same-file collisions | 0 |
| Prevented lost edit events | 0 |
| Additional coordinated blocks | 0 |
| Additional denied writes | 0 |
| Additional scope mismatches | 0 |
| Additional stale intents | 0 |
| Additional timeouts | 0 |
| Additional long idle periods | 0 |
| Additional false blocks | 0 |
| Additional manual interventions | 0 |
| Additional coordination friction events | 0 |
| Additional wall time ms | -519 |

## Mode Metrics

| Metric | Stateful | Awareness | No-state |
| --- | ---: | ---: | ---: |
| Artifact pairs | 15 | 15 | 15 |
| Scored pairs | 15 | 15 | 15 |
| Available valid functional score | 0.144 | 0.144 | 0.144 |
| Raw manifest functional score | 0.144 | 0.144 | 0.144 |
| Missing artifacts | 0 | 0 | 0 |
| Setup error pairs | 0 | 0 | 0 |
| Unknown pairs | 0 | 0 | 0 |
| Infra loss rate | 0.000 | 0.000 | 0.000 |
| Uncoordinated same-file collisions | 0 | 0 | 0 |
| Lost edit events | 0 | 0 | 0 |
| Coordinated blocks | 0 | 0 | 0 |
| Denied writes | 0 | 0 | 0 |
| Authorization warnings | 0 | 0 | 0 |
| Warned writes applied | 0 | 0 | 0 |
| Wait events | 0 | 0 | 0 |
| Preserved edit count | 5 | 5 | 5 |
| Missing expected line count | 27 | 27 | 27 |
| False block count | 0 | 0 | 0 |
| Missed conflict count | 0 | 0 | 0 |
| Manual intervention count | 0 | 0 | 0 |
| Time to converge ms | n/a | n/a | n/a |

