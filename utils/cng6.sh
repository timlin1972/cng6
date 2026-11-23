#!/bin/bash
# cng6 管理腳本: 在 tmux 裡執行 cng6, 開機自動啟動、restart 指令重新編譯並重啟。
#
#   utils/cng6.sh start    在背景(tmux session "cng6")啟動
#   utils/cng6.sh attach   看 TUI(離開但不結束: 按 Ctrl-b 再按 d)
#   utils/cng6.sh stop     結束
#   utils/cng6.sh status   是否在執行
#   utils/cng6.sh run      (給 tmux 用) 編譯 + 執行的迴圈
#
# 開機自動啟動: crontab 加入
#   @reboot /home/timlin/cng6/utils/cng6.sh start
#
# run 迴圈:
# - 每次啟動前先從目前分支的 upstream 快轉更新, 再編譯(更新失敗也照樣編譯本機的程式碼)
# - 更新後這個腳本本身變了: 改用新的腳本重新執行
# - 編譯失敗時繼續執行上一次成功編譯的版本
# - cng6 結束碼 3 (restart 指令): 重新編譯並啟動
# - 結束碼 0 (quit): 結束迴圈, tmux session 也跟著關閉
# - 其他(當掉): 等 3 秒再重新啟動, 避免一直重啟吃滿 CPU

set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/target/debug/cng6"
# 測試用: 可以換 session 名稱與執行目錄(settings.conf、桌布所在的地方)
SESSION="${CNG6_SESSION:-cng6}"
WORKDIR="${CNG6_WORKDIR:-$ROOT}"
# crontab 的 PATH 很少, 補上 cargo 的位置
export PATH="$HOME/.cargo/bin:$PATH"
# crontab 沒有語系設定, tmux 才能正確顯示中文
export LANG="${LANG:-C.UTF-8}"

running() {
    tmux has-session -t "$SESSION" 2>/dev/null
}

build() {
    echo "[cng6] 編譯中…"
    # 降低優先權與並行數, 減少對 SD 卡的壓力
    if nice -n 10 ionice -c3 cargo build -j 2; then
        return 0
    fi
    if [ -x "$BIN" ]; then
        echo "[cng6] 編譯失敗, 繼續執行上一次的版本"
        sleep 3
        return 0
    fi
    return 1
}

update() {
    echo "[cng6] 從 Git 更新程式碼…"
    # 在背景執行, 不能停下來問帳號密碼或確認主機; 網路不通時也不要卡太久
    if GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND="ssh -o BatchMode=yes -o ConnectTimeout=15" \
        timeout 120 git -C "$ROOT" pull --ff-only; then
        return 0
    fi
    echo "[cng6] Git 更新失敗, 使用本機的程式碼"
    return 1
}

run_loop() {
    # bash 執行時已經讀進這個腳本; 被 git 更新後要重新執行才會用到新的內容
    local self="$ROOT/utils/cng6.sh"
    local sum
    sum="$(cksum < "$self")"
    while true; do
        cd "$ROOT" || exit 1
        update
        if [ "$(cksum < "$self")" != "$sum" ]; then
            echo "[cng6] 管理腳本更新了, 用新的腳本重新執行"
            exec "$self" run
        fi
        if ! build; then
            echo "[cng6] 編譯失敗而且沒有可執行的版本, 30 秒後重試 (Ctrl-C 結束)"
            sleep 30
            continue
        fi
        cd "$WORKDIR" || exit 1
        CNG6_SUPERVISED=1 "$BIN" "$ROOT/startup.cng"
        code=$?
        case $code in
            0)
                echo "[cng6] 已結束"
                break
                ;;
            3)
                echo "[cng6] 重新啟動"
                ;;
            *)
                echo "[cng6] 異常結束 (結束碼 $code), 3 秒後重新啟動"
                sleep 3
                ;;
        esac
    done
}

main() {
    case "${1:-}" in
        start)
            if running; then
                echo "cng6 已經在執行 (utils/cng6.sh attach 查看)"
                exit 0
            fi
            tmux new-session -d -s "$SESSION" -x 200 -y 50 -c "$ROOT" \
                -e "CNG6_SESSION=$SESSION" -e "CNG6_WORKDIR=$WORKDIR" "$ROOT/utils/cng6.sh run"
            echo "cng6 已在背景啟動 (utils/cng6.sh attach 查看)"
            ;;
        attach)
            if ! running; then
                echo "cng6 沒有在執行 (utils/cng6.sh start 啟動)"
                exit 1
            fi
            exec tmux attach -t "$SESSION"
            ;;
        stop)
            if ! running; then
                echo "cng6 沒有在執行"
                exit 0
            fi
            # 先請 cng6 正常結束(寫入歷史等), 不行再強制關閉
            tmux send-keys -t "$SESSION" Escape C-u "/quit" Enter
            for _ in $(seq 20); do
                running || { echo "cng6 已結束"; exit 0; }
                sleep 0.5
            done
            tmux kill-session -t "$SESSION"
            echo "cng6 已強制結束"
            ;;
        status)
            if running; then echo "cng6 執行中 (tmux session: $SESSION)"; else echo "cng6 沒有在執行"; fi
            ;;
        run)
            run_loop
            ;;
        *)
            sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
            exit 1
            ;;
    esac
}

# 放在同一行: bash 會先讀完整行才執行, 腳本在執行中被 git 改掉也不會接著讀到改過的內容
main "$@"; exit
