# StatefulBench parallel-on OMP 업그레이드 설계
> **상태: Superseded.** OMP 17.0.4 단독 상향은 `AgentBusyError`를 해결하지 못했습니다. 이 design은 [2026-07-18-statefulbench-parallel-on-context-injection-design.md](2026-07-18-statefulbench-parallel-on-context-injection-design.md)로 대체되었습니다.


## 목표

StatefulBench real-world Docker 런타임의 OMP를 17.0.4로 고정하고, `requests`의 `parallel-on` 1회 평가를 정상 완료한다.

## 확인된 실패

기존 `linux/arm64` 이미지의 OMP 16.4.2에서 `requests`와 `jsonschema`의 모든 `parallel-on` 에이전트가 첫 프롬프트 시작 직후 `AgentBusyError: Agent is already processing`으로 종료됐다. 같은 이미지의 `sequential`과 `parallel-off` 행은 통과했다.

## 변경 범위

`crates/stateful-bench/docker/statefulbench-realworld.Dockerfile`의 `OMP_VERSION`만 `16.4.2`에서 `17.0.4`로 변경한다. Stateful 확장, benchmark runner, 평가기, corpus, 자격 증명 복사 로직은 변경하지 않는다.

## 검증 흐름

1. 변경된 Dockerfile로 `linux/arm64` 이미지를 새로 빌드한다.
2. 이미지 ID, 플랫폼, repository digest, OMP 17.0.4를 검사한다.
3. 새 이미지 identity에 맞춰 `requests`를 재qualification한다.
4. 새 출력 디렉터리에서 다음 범위를 실행한다.
   - repository: `requests`
   - arm: `parallel-on`
   - trials: `1`
   - model: `openai-codex/gpt-5.6-terra`
   - thinking: `high`
   - timeout: `3600`초
5. 결과와 진단을 확인한다.

## 완료 기준

유일한 scheduled row가 다음 조건을 모두 만족해야 한다.

- `cleared`가 `true`다.
- 모든 task agent와 final agent가 정상 종료한다.
- 모든 evaluator와 upstream suite가 통과한다.
- arm container가 제거된다.
- `coordination_metrics`가 비어 있지 않고 `protocol_version`이 `stateful.v2`다.
- 결과의 qualification image identity가 실행 image identity와 일치한다.

## 실패 처리

OMP 17.0.4에서도 `AgentBusyError` 또는 다른 uncleared 결과가 발생하면 버전 상향 접근이 실패한 것으로 판단한다. benchmark 전용 우회나 Stateful 확장 변경을 자동으로 추가하지 않고, 새 증거를 바탕으로 별도 설계를 승인받는다.
