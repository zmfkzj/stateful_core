# SDD Progress

Base commit: 0391cc9
Task 1: complete (commits 0391cc9..33a1c07, review clean)
Task 2: complete (commits 33a1c07..67dff27, review clean)
Task 3: complete (verification passed: stateful-bench OMP parser 1 test, stateful-cli install_global 29 tests)

## Stateful approval auto-approve

Base commit: 5ab028d
Task 1: complete (commits 5ab028d..6b3ee03, review clean)
Task 2: complete (commits 6b3ee03..d7a2b0b, review clean)
Task 3: complete (verification passed: stateful-cli install_global 29 tests; final review clean after 4674cfa)

## process_find safe output fields

Base commit: fd25e30
Task 1: complete (commits fd25e30..162d58e, review clean)
Task 2: complete (commits 162d58e..2e0a776, review clean after selector guard fix)
Task 3: complete (verification passed: process_find tests 7+4, install_global process_find schema test 1; final review clean after 2e0a776)

## ProgramBench integration

Base commit: 3ec7c90
Task 1: complete (commits 3ec7c90..4c127c1, review clean after compare path fix)
Task 2: complete (commits 4c127c1..a1907de, review clean after schema drift fix)
Task 3: complete (commits a1907de..d953d8d, review clean after per-instance rounding regression)
Task 4: complete (commits d953d8d..e9f8ade, review clean after token parser regression fix)
Task 5: complete (commits e9f8ade..6344ed0, review clean)
Task 6: complete (docs updated; verification passed: cargo fmt, stateful-bench programbench test, full stateful-bench test)
Final review fixes: ProgramBench run/eval now execute instance-level submissions, adapters execute inside the ProgramBench container with stateful setup, subagent usage is observation-derived, comparison deltas require common instances, and ProgramBench helper types are re-exported.

## OMP background bash polling

Base commit: bf21d21
Task 1: complete (commits bf21d21..44dde05, review clean after metadata assertion fix)
Task 2: complete (commits 44dde05..9f9fa85, review clean after hook allow and failure-status fixes)
Task 3: complete (commits 9f9fa85..c6a4784, review clean)

## glob default allowlist

Base commit: 5cae9b9
Task 1: complete (commits 5cae9b9..18c8685, review clean)

## OMP generated command tool removal

Base commit: 58bd33b
Task 1: complete (commits 58bd33b..304eeaa, review clean)

## Benchmark overhead reduction

Base commit: df00191
Task 1: complete (commits df00191..0756faa, review clean)
Task 2: complete (commits 0756faa..1c66612, review clean)

## Sandbox sequence

Base commit: 3ed94e3
Task 1: complete (commits 3ed94e3..12421d8, review clean after lint narrowing)
Task 2: complete (commits 12421d8..8ed052e, review clean)
Task 3: complete (commits 8ed052e..d6752a6, review clean after direct-profile fix)
Task 4: complete (commit 0548acc, review clean; unrelated intervening benchmark commits ignored)
Task 5: blocked in current live Pi process (tests pass; exact `--sequence` Bash smoke still uses stale in-memory OMP preflight after installed CLI/profile update)

## Agent-only wall time

Base commit: 808b3a5
Task 1: DeNovo Rust metrics complete (tests pass; review clean)
Task 2: DeNovo Python metrics complete (tests pass after timeout fix; review clean)
Task 3: ProgramBench Rust metrics complete (tests pass; review clean)
Task 4: ProgramBench Python metrics complete (tests pass; review clean)
Task 5: Timing documentation complete (review clean)

## Collaboration context enrichment

Base commit: cd54c3d
Task 1: complete (commits cd54c3d..e5a1d03, review clean)
Task 2: complete (commits e5a1d03..be0c9c3, review clean)
Task 7: complete (commits be0c9c3..5ac0149, review clean after OMP helper fix)
Task 8: complete (commits 5ac0149..febc96f, review clean after integration/fingerprint fix)
Task 3: complete (commits febc96f..4b494d3, review clean)
Task 4: complete (commits 4b494d3..9c00c3b, review clean after compressed-next-action fix)
Task 6: complete (commits 9c00c3b..3d01155, review clean)
Task 5: complete (commits 9c00c3b..fb90eb8, review clean after expired-active-row fix)

## Stateful-on advantage

Base commit: 16fb44e
Task 1.1: complete (commits 16fb44e..299a54c, review clean after payload-nested wait fix)
Task 1.2: complete (commits 299a54c..8214de7, review clean)
Task 2.1: complete (commits 8214de7..d00b14d, review clean)
Task 2.2: complete (commits d00b14d..6ab2ecf, manual review clean after subagent quota exhaustion)
Task 3.1: complete (commit e0bba62, manual review clean; denovo 27/27, cli 17/17)
Task 3.2: complete (commit f2ac9ad, manual review clean; docs-only verified by reread)
Phase 4 gate: stopped findings-sharing implementation (commit 73870d6; no re-derivation evidence yet)


## Stateful Coordination Task-Graph Benchmark

Base commit: 73870d6
Task 1: complete (commit 055eacc; source, math, and consistency reviews clean after corrections)

## StatefulBench implementation

Base commit: 3a282ba1
Gate 2.1 fixed cursor storage: complete (commit c0baf45; review clean after atomic-migration and zero-limit fixes)
Gate 1.1 OMP hard-turn cap: complete (fork commit 67986f87b; focused TDD and independent review clean)
Gate 2.2 HTTP event cursor routes: complete (commit 99a43dd; review clean after legacy-query compatibility fixes)
Gate 1.2 OMP startup controls: complete (fork commits ca8cbbcd4..adb6e9988; review clean after sealed-mode hardening)

## StatefulBench lite

Base commit: 3a282ba
Task-graph gates 1-10 and the OMP fork: cancelled by user; commits c0baf45..f6b6823 removed from dev (archived at archive/task-graph-gate2).
Lite harness (5 parallel tasks + final review, 3 arms, efficiency metrics): implemented; final commit pending

## StatefulBench Docker runtime

Base commit: d254d47
Task 1: complete (commits d254d47..cd95a56, review clean; 8 focused tests; native linux/arm64 image built and inspected)
Task 2: complete (commits cd95a56..806fbb6, review clean after immutable-image, container-attestation, and stale-receipt fixes; 43 focused tests; Requests qualified in Docker)
Task 3: complete (commit 7ce1fa7, review clean after container-command completion and immutable-ID cleanup; 9 focused tests)
Task 4: complete (commit 8ed35d6, review clean after host-fallback removal, Docker-client reap, and exit-record race fixes; 35 focused tests)
Task 5: complete (commits 8ed35d6..f64f7ae, review clean after SQLite/path/redaction/provenance/cleanup hardening; 33 focused tests)
Task 6: complete (commit 3b1ff49, review clean; credit-free all-three-arm Docker gate passed)
Final hardening: complete (commits 4be3bea..916d499; final review ready to integrate after platform, provenance, staging, descendant, grading, timing, and cleanup fixes)
Definitive verification: Docker 40 passed/1 opt-in skip, real-world 118 passed, lite 20 passed, live Docker E2E passed, Requests qualified against image aaebfb256063; dev pushed at 916d499.