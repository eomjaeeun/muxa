# 이 포크에서 추가/수정한 것들

upstream [`Open330/muxa`](https://github.com/Open330/muxa)에 없는, 이 개인 포크(`personal/watch`)만의 주요 변경사항. 전체 변경 이력은 `git log`를 참고.

## 추가한 기능

### Unread(안읽음) 표시
에이전트는 turn이 끝나면 전부 `Idle`로 표시되는데, "idle이고 이미 답을 확인함"과 "idle인데 아직 안 봄"이 구분이 안 됐다. 에이전트 여러 개를 동시에 굴릴 때 매번 pane을 열어봐야 했던 문제.

- 안 읽은 idle 행을 전용 색으로 표시, `u`/`U`로 개별/전체 읽음 처리
- 읽은 위치는 `$XDG_DATA_HOME/muxa/watch-read.json`에 저장돼서 재시작해도 유지됨
- 폰트가 상태 아이콘을 두 칸으로 잘못 그리는 문제 때문에 `[ui] icons = "narrow"` 옵션도 같이 추가

### Preview에서 Tab으로 대상 pane 고르기
한 window에 agent가 여러 개 있으면 메시지(`m`)나 점프(Enter)가 어느 pane을 향하는지 알 방법이 없었다. `Tab`으로 window 행이 가리키는 pane을 명시적으로 고르고, 그 선택이:

- live layout(미니 프리뷰) 하이라이트, `m`이 보내는 대상, Enter가 점프하는 대상 전부에 동일하게 적용됨
- `Tab`을 누른 tmux 자체 focus와도 동기화됨
- `watch` 재시작(점프 시 항상 발생)에도 선택이 유지되도록 디스크에 저장됨

### `muxa-bridge` 스킬
docker 컨테이너 안에서 도는 claude 세션을 호스트 muxa가 tmux pane처럼 추적하게 만드는 socat 브리지 구성. `skills/muxa-bridge/SKILL.md`에 통째로 문서화돼 있고, 이 repo를 clone한 뒤 "muxa-bridge skill 추가합시다"라고만 하면 그 자리에서 설치된다.

## 고친 버그

### Preview 창의 캡처가 실제 agent 화면 폭과 안 맞던 것
tmux 캡처를 mosaic/inspector에 그릴 때 폭 계산이 문자 수 기준이라 한글이 반으로 잘리고, 박스 테두리까지 줄바꿈되고, 구분선이 깨지는 등 렌더링이 전반적으로 어긋나 있었다. 셀(cell) 단위로 폭을 다시 계산하고, 실제 텍스트만 줄바꿈하고 테두리/padding은 자르도록 정리.

### 한국어 입력 · 상태 표시 관련 잡버그
- macOS에서 한글 조합 중 입력(IME)이 muxa watch의 테이블 필터로 새어 들어가던 것
- roster의 헤더/윈도우 행/pane 행이 서로 다른 포맷을 써서 컬럼이 안 맞던 것
- `Ctrl-B`가 tmux 자체 prefix인데 muxa가 가로채서 mailbox를 열어버리던 것
