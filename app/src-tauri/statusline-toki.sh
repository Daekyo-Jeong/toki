#!/bin/sh
# Toki 배터리 피드 — Claude Code가 매 턴 statusLine 명령의 stdin으로 흘려주는
# rate_limits에서 5시간 사용률만 뽑아 ~/.toki/usage-pct 에 적는다.
#
# 왜 이 경로인가: /api/oauth/usage 직접 폴링은 비공식 클라이언트에 429를 준다.
# statusLine payload는 Messages API 응답에 얹혀 오는 값이라 추가 호출이 0이고,
# Claude Code가 보여주는 %와 정확히 같은 수다.
#
# 설계 원칙 — statusLine은 스트리밍 중 초당 여러 번 실행된다:
#   * 이 스크립트는 프로세스를 하나도 띄우지 않는다 (순수 sh 파라미터 확장 +
#     리다이렉트). 타임스탬프도 안 적는다 — 신선도는 Rust 쪽이 mtime으로 판단.
#   * 사용자의 원래 statusLine은 반드시 그대로 이어 실행하고 stdout을 통과시킨다.
#     우리 때문에 남의 상태줄이 죽으면 안 된다.
#
# 본문은 고정이다 — 인스톨러(statusline_installer.rs)가 이 파일을 그대로
# ~/.toki/statusline-toki.sh 로 내보내고, 사용자의 원래 statusLine 명령은
# 옆의 ~/.toki/statusline-next.sh 에 따로 둔다(있으면 실행, 없으면 끝).
# 경로를 스크립트 안에 치환해 넣지 않는 이유: 원래 명령이 셸 한 줄짜리라
# 따옴표 처리가 위험하고, 스크립트가 머신마다 달라지면 검증이 안 된다.

payload=
while IFS= read -r line || [ -n "$line" ]; do
  payload="${payload}${line}
"
done
payload=${payload%?}

# rate_limits가 실린 턴에서만 값을 갱신한다.
case "$payload" in
  *'"five_hour"'*)
    rest=${payload#*'"five_hour"'}
    rest=${rest#*'"used_percentage"'}
    rest=${rest#*:}
    rest=${rest#"${rest%%[![:space:]]*}"}
    val=${rest%%[!0-9.]*}
    case "$val" in
      ''|.|*.*.*) ;;                       # 빈 값·이상한 형태는 버린다
      *) printf '%s' "$val" > "$HOME/.toki/usage-pct" 2>/dev/null || : ;;
    esac
    ;;
esac

# 사용자의 원래 statusLine(있으면)으로 payload를 그대로 넘기고 출력도 통과.
NEXT="$HOME/.toki/statusline-next.sh"
if [ -s "$NEXT" ]; then
  printf '%s' "$payload" | /bin/sh "$NEXT"
fi
exit 0
