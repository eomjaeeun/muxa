---
name: muxa-bridge
description: "컨테이너 안에서 도는 claude를 호스트 muxa가 호스트 pane처럼 추적하게 만드는 소켓 브리지. 재부팅 후 복구, 새 컨테이너 추가, 컨테이너 claude가 muxa watch에 안 보일 때 사용."
---

# muxa 컨테이너 브리지

호스트 muxad가 docker 컨테이너(`je-dev`, `je-new`) 안의 claude 세션을 **호스트 tmux pane에 붙은 에이전트로** 추적하게 만드는 구성. muxa 공식 기능이 아니라 수동 조립이므로 muxa 업그레이드 후에는 동작을 다시 확인할 것.

## 왜 필요한가

muxa IPC는 유닉스 소켓(= 파일)이고 컨테이너는 파일시스템이 분리돼 있어서, 컨테이너 안 프로세스는 호스트의 `muxa.sock`을 열 수 없다. 컨테이너↔호스트를 잇는 건 네트워크뿐이므로 socat으로 **파일 → TCP → 파일** 변환을 한다.

```
컨테이너 /tmp/muxa.sock ──TCP 172.18.0.1:47653──▶ 호스트 /run/user/1001/muxa.sock
        (socat, 컨테이너)                              (socat, 호스트)
```

## 구성 요소

| 위치 | 내용 |
| --- | --- |
| docker 네트워크 | `muxa-br` — 게이트웨이 `172.18.0.1`. 대상 컨테이너는 기존 `bridge`와 **이중 연결** |
| 호스트 유닛 | `~/.config/systemd/user/muxa-bridge-host.service` |
| 컨테이너 유닛 | `~/.config/systemd/user/muxa-bridge-container@.service` (템플릿, `@je-dev` / `@je-new`) |
| 컨테이너 안 | `/usr/local/bin/socat`, `/usr/local/bin/muxa` (호스트 바이너리 복사본) |
| 컨테이너 안 | `~/.claude/settings.json`의 hook 7개 (백업: `settings.json.bak-muxa`) |
| 호스트 | `~/.zshrc`의 `jein` / `jedev` / `jenew` 함수 |

## 상태 확인

```bash
systemctl --user is-active muxa-bridge-host muxa-bridge-container@je-dev muxa-bridge-container@je-new
docker exec je-dev ls -la /tmp/muxa.sock          # 소켓 파일 존재?
docker exec je-dev sh -c 'echo "{\"protocol\":6,\"kind\":\"snapshot\"}" | socat - UNIX-CONNECT:/tmp/muxa.sock'
```

마지막 명령이 에이전트 목록 JSON을 뱉으면 전체 경로가 살아 있는 것. `protocol mismatch` 에러가 나와도 **왕복은 성공**한 것이므로 버전 숫자만 맞추면 된다.

## 재부팅 후 복구

유닛은 `enable` 되어 있지만 `Linger=no`라서 **부팅 시점이 아니라 로그인할 때** 뜬다. 또 두 컨테이너 모두 `RestartPolicy=no`라 자동 기동되지 않는다.

```bash
docker start je-dev je-new        # 1. 컨테이너 먼저
                                   # 2. 유닛은 10초마다 재시도하므로 자동으로 붙음
systemctl --user is-active muxa-bridge-host muxa-bridge-container@je-dev
```

유닛이 안 떠 있으면 `systemctl --user start muxa-bridge-host muxa-bridge-container@je-dev`.

로그인 전에도 뜨게 하려면(sudo 필요): `sudo loginctl enable-linger eomjaeeun`

**주의:** 컨테이너를 `docker rm` 후 재생성하면 `/usr/local/bin`의 바이너리와 `settings.json` hook이 전부 사라진다. 아래 "새 컨테이너 추가"를 처음부터 다시 할 것.

## 새 컨테이너 추가

`<NAME>`을 컨테이너 이름으로 바꿔서 순서대로 실행.

```bash
# 1. 전용 네트워크 연결 (실행 중인 컨테이너에도 가능, 기존 연결 유지됨)
docker network connect muxa-br <NAME>

# 2. 바이너리 복사 — snap docker는 /usr/bin, /tmp를 못 보므로 홈 경유 필수
cp /usr/bin/socat ~/socat.tmp && docker cp ~/socat.tmp <NAME>:/usr/local/bin/socat && rm ~/socat.tmp
cp ~/.cargo/bin/muxa ~/muxa.tmp && docker cp ~/muxa.tmp <NAME>:/usr/local/bin/muxa && rm ~/muxa.tmp
docker exec <NAME> sh -c 'socat -V | head -1; muxa --version'

# 3. settings.json 백업 후 hook 7개 추가
docker exec <NAME> cp ~/.claude/settings.json ~/.claude/settings.json.bak-muxa
#    기존 내용을 보존한 채 아래 hooks 블록만 병합해서 docker cp 로 써넣는다
docker exec <NAME> grep -c "muxa hook claude" ~/.claude/settings.json   # 7 이어야 함

# 4. 유닛 기동
systemctl --user enable --now muxa-bridge-container@<NAME>.service

# 5. 접속 함수 추가 (~/.zshrc)
#    je<X>() { jein <NAME> "$@"; }

# 6. peer 대화 활성화 (아래 "pane 끼리 대화시키기" 참고)
docker exec <NAME> sh -c 'grep -q MUXA_SOCKET ~/.zshrc || echo "export MUXA_SOCKET=/tmp/muxa.sock" >> ~/.zshrc'
```

3단계의 hooks 블록 (7개 이벤트 전부 같은 모양):

```json
"hooks": {
  "SessionStart":     [{"hooks":[{"type":"command","command":"MUXA_SOCKET=/tmp/muxa.sock muxa hook claude --event session_start"}]}],
  "SessionEnd":       [{"hooks":[{"type":"command","command":"MUXA_SOCKET=/tmp/muxa.sock muxa hook claude --event session_end"}]}],
  "UserPromptSubmit": [{"hooks":[{"type":"command","command":"MUXA_SOCKET=/tmp/muxa.sock muxa hook claude --event user_prompt_submit"}]}],
  "PreToolUse":       [{"hooks":[{"type":"command","command":"MUXA_SOCKET=/tmp/muxa.sock muxa hook claude --event pre_tool_use"}]}],
  "PostToolUse":      [{"hooks":[{"type":"command","command":"MUXA_SOCKET=/tmp/muxa.sock muxa hook claude --event post_tool_use"}]}],
  "Notification":     [{"hooks":[{"type":"command","command":"MUXA_SOCKET=/tmp/muxa.sock muxa hook claude --event notification"}]}],
  "Stop":             [{"hooks":[{"type":"command","command":"MUXA_SOCKET=/tmp/muxa.sock muxa hook claude --event stop"}]}]
}
```

## 컨테이너 claude 띄우는 법

```bash
jedev        # je-dev
jenew        # je-new
jein <NAME>  # 임의 컨테이너
```

**반드시 이 함수로 들어가야 한다.** 그냥 `docker exec -it je-dev zsh`로 들어가면 이벤트는 호스트에 도착하지만 `pane=null`이라 muxa watch에서 보이지 않는다.

이미 예전 방식으로 띄운 세션은 claude 종료 → `exit` → `jedev` → `claude --resume <id>` 로 다시 붙이면 제자리를 찾는다.

## pane 끼리 대화시키기 (host claude ↔ 컨테이너 claude)

브리지가 붙은 컨테이너 claude는 **같은 tmux window의 호스트 claude와 muxa peer**가 된다.
`muxa peers`로 서로 보이면 이미 절반은 된 것이고, 다음 두 가지가 더 있어야 실제로 대화가 된다.

**(a) 컨테이너 안에 `MUXA_SOCKET` 환경변수** — 없으면 `muxa msg`/`muxa peers`가
기본값 `/tmp/muxa-1001.sock`을 보고 조용히 실패한다.

```bash
docker exec <NAME> sh -c 'grep -q MUXA_SOCKET ~/.zshrc || echo "export MUXA_SOCKET=/tmp/muxa.sock" >> ~/.zshrc'
```

**(b) 컨테이너 claude 에 muxa MCP 서버** — 이게 있어야 `muxa_call_peer` /
`muxa_inbox` / `muxa_reply` 툴이 생긴다. 없어도 Bash 로 `muxa msg` CLI 를 쓸 수는 있지만
프로토콜을 모르는 상태라 대화가 잘 안 굴러간다.

```bash
docker exec <NAME> cp ~/.claude.json ~/.claude.json.bak-muxa-mcp
docker exec <NAME> python3 -c '
import json
p="/home/user/.claude.json"; d=json.load(open(p))
d.setdefault("mcpServers",{})["muxa"]={"type":"stdio","command":"muxa","args":["mcp"],
  "env":{"MUXA_SOCKET":"/tmp/muxa.sock"}}
json.dump(d,open(p,"w"),indent=2)'
```

MCP 서버는 claude 시작 시점에 뜨므로 **이미 떠 있는 컨테이너 claude 는 재시작해야**
툴이 생긴다 (`/exit` → `claude --resume <id>`).

확인:

```bash
docker exec -e MUXA_SOCKET=/tmp/muxa.sock -e TMUX_PANE=%<컨테이너pane> <NAME> muxa peers
# room @N · self claude_code@%40 · ... / %1  claude@%1  working
```

호스트/컨테이너 `muxa --version`이 **같아야** 한다 (IPC 프로토콜 버전).

### 실제로 대화 거는 법

`@peer`는 **가로채는 장치가 아니라 약속**이다. hook은 전혀 관여하지 않는다 —
`@peer` 문자열은 `crates/muxa-cli/src/mcp.rs`에만 있고 hook/daemon 쪽에는 없다.
모델이 instructions를 읽고 스스로 `muxa_call_peer`를 부르는 것이라 **안 부를 수도 있다.**

모델에게 그 약속을 알려주는 채널이 두 개인데, **컨테이너에는 하나뿐**이다:

| 채널 | 호스트 | 컨테이너 |
| --- | --- | --- |
| muxa MCP 서버 instructions | O | O |
| `muxa init`이 CLAUDE.md에 쓰는 `@peer/@muxa-peer` 줄 | O | **X** |

그래서 트리거 강도가 다르다:

```
호스트 pane:     @peer 지금 뭐 하고 있었는지 물어봐줘
컨테이너 pane:   muxa_call_peer 써서 물어봐줘 — 지금 뭐 하고 있었는지
```

컨테이너에서 `@peer`만 쳐서 아무 일도 안 일어나면 버그가 아니라 이것. 툴 이름을
직접 부르면 확실하다. 통일하고 싶으면 컨테이너에도 CLAUDE.md 줄 +
`~/.config/muxa/agent-integration/bootstrap.md`를 넣으면 된다 (아직 안 해둠).

참고로 Claude Code에서 `@`는 파일 멘션 메뉴를 연다. muxa는 tool만 노출하고
리소스를 안 만들어서 `@` 후보에 `peer`가 뜨지 않는다 — 정상이다. Esc로 메뉴만 닫고
텍스트는 그대로 두면 된다.

안 갔는지 확인: `muxa msg list --json`이 `[]`면 요청 자체가 안 만들어진 것.

## 자주 틀리는 것

| 증상 | 원인 |
| --- | --- |
| 이벤트가 아예 안 감 | `MUXA_SOCKET=/tmp/muxa.sock` 누락. 기본값이 `/tmp/muxa-1001.sock`이라 조용히 아무 데도 안 간다 |
| 이벤트는 오는데 watch에 안 보임 | `$TMUX_PANE` 미전달 → `pane=null`. watch UI는 paneless 에이전트를 기본 숨김 |
| 5개 세션이 한 칸에 뭉침 | 컨테이너 `~/.zshrc`에 `TMUX_PANE`을 고정값으로 넣은 경우. pane마다 값이 달라야 하므로 **exec 시점에** 넘겨야 한다 |
| `docker cp: no such file or directory` | snap docker라 `/usr/bin`, `/tmp`가 안 보임. 홈 디렉터리 경유 |
| hook 설치했는데 조용함 | hook은 **사건이 생겨야** 발동한다. 유휴 세션은 다음 프롬프트/응답까지 안 올라옴 |

## 보안 경계

muxa IPC 소켓에는 **인증이 없다** (`PROTOCOL.md`: "Treat socket access as equivalent to shell access for that user"). `send_prompt`로 호스트 tmux 아무 pane에나 키 주입이 가능하다. 0600 파일 권한이 유일한 자물쇠인데 이 구성은 그걸 TCP로 바꾼다.

그래서 호스트 유닛의 `bind=172.18.0.1`이 **필수**다. 이걸 빼거나 기본 bridge(`172.17.0.1`)에 열면 거기 붙은 `je-exp-*` 실험 컨테이너 31개가 전부 호스트 tmux를 조종할 수 있게 된다. 퍼징 타겟이 도는 컨테이너들이므로 절대 열지 말 것.

현재 접근 가능 범위: `muxa-br`에 붙은 컨테이너(`je-dev`, `je-new`) 안의 **모든 프로세스**.

## 제거

```bash
systemctl --user disable --now muxa-bridge-host muxa-bridge-container@je-dev muxa-bridge-container@je-new
rm ~/.config/systemd/user/muxa-bridge-{host,container@}.service
systemctl --user daemon-reload
docker network disconnect muxa-br je-dev; docker network disconnect muxa-br je-new
docker network rm muxa-br
# 컨테이너 안: settings.json.bak-muxa 복원, /usr/local/bin/{muxa,socat} 삭제
```
