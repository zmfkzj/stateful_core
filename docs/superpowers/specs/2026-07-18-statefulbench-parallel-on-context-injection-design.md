# StatefulBench parallel-on 초기 컨텍스트 주입 수정 설계

## 목표

stateful_core의 OMP 확장이 세션 시작 컨텍스트를 별도 turn 없이 최초 모델 turn에 포함하도록 수정하고, OMP 17.0.4 기반 `requests` `parallel-on` 1회 평가를 정상 완료한다.

## 확인된 원인

OMP 16.4.2와 17.0.4 모두 `parallel-on`의 모든 에이전트가 첫 프롬프트에서 `AgentBusyError: Agent is already processing`으로 종료됐다. stateful_core의 `stateful-omp-extension.js`는 `session_start` 중 초기 컨텍스트를 `deliverAs: "nextTurn", triggerTurn: true`로 보낸다. OMP 세션은 이 시점에 `isStreaming`을 아직 true로 노출하지 않지만 첫 프롬프트 처리는 이미 진행 중이므로, 새 agent-initiated turn 시작이 기존 프롬프트와 충돌한다.

OMP 17.0.4의 `sendCustomMessage` 계약상 non-streaming 상태에서 `deliverAs: "nextTurn", triggerTurn: false`는 메시지를 agent/session 상태에 append하고 새 turn을 시작하지 않는다. 따라서 session-start handler가 반환된 뒤 시작되는 최초 프롬프트가 해당 컨텍스트를 읽을 수 있다.

## 변경 범위

- `crates/stateful-cli/assets/stateful-omp-extension.js`
  - `deliverContext`가 turn 시작 여부를 받게 한다.
  - 기본값은 기존 동작인 `true`다.
  - `session_start`의 최초 컨텍스트 전달만 `false`를 사용한다.
- `crates/stateful-cli/assets/stateful-omp-extension.test.mjs`
  - session-start 컨텍스트가 `deliverAs: "nextTurn", triggerTurn: false`로 전달됨을 검증한다.
  - render와 acknowledgement가 각각 한 번 수행되는 기존 전달 계약을 유지한다.
- `crates/stateful-bench/docker/statefulbench-realworld.Dockerfile`
  - OMP 17.0.4 상향을 유지한다.

Stateful 서버, runner, evaluator, corpus, 자격 증명 복사 로직과 SSE·예약 알림의 turn 동작은 변경하지 않는다.

## 데이터 흐름

1. OMP가 `session_start`를 발생시킨다.
2. Stateful 확장이 세션 identity를 등록하고 초기 context를 render한다.
3. 확장이 rendered context를 `nextTurn` 메시지로 append하되 새 turn을 시작하지 않는다.
4. 확장이 context delivery를 acknowledge하고 notification stream을 시작한다.
5. OMP가 원래의 첫 프롬프트를 계속 처리하며 appended Stateful context를 함께 읽는다.
6. 이후 SSE invalidation과 reservation-ready 알림은 기존 `triggerTurn: true` 동작을 유지한다.

## 검증

1. 기존 session-start 확장 테스트가 변경 전 코드에서 실패하고 수정 후 통과하는 red/green을 확인한다.
2. 전체 OMP 확장 asset 테스트와 관련 stateful-cli 테스트를 실행한다.
3. OMP 17.0.4 `linux/arm64` 이미지를 다시 빌드·검사한다.
4. credit-free Docker E2E의 세 arm을 통과시킨다.
5. 새 image identity로 `requests`를 재qualification한다.
6. 새 출력 디렉터리에서 `requests`, `parallel-on`, 1 trial, `openai-codex/gpt-5.6-terra`, thinking `high`, timeout `3600`초를 실행한다.

## 완료 기준

유일한 model-backed row가 다음을 모두 만족해야 한다.

- `cleared == true`
- 모든 task agent와 final agent의 `exit_code == 0`
- evaluator, post suite, upstream suite 통과
- container 제거
- `coordination_metrics.protocol_version == "stateful.v2"`
- qualification image identity와 runtime image identity 일치
- `runtime.platform == "linux/arm64"`
- summary aggregate의 `row_count == 1`, `cleared_count == 1`

## 실패 처리

수정 후에도 row가 uncleared면 추가 우회를 만들지 않는다. 결과, agent stderr, diagnostics와 coordination 상태를 보존하고 새 원인에 대한 별도 설계를 승인받는다.
