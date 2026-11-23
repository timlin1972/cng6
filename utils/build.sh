#!/bin/bash
# 在 Pi 上安全地編譯: 低優先權、一次只編一個、限制記憶體。
#
#   utils/build.sh                 cargo build
#   utils/build.sh clippy --all-targets
#   utils/build.sh test
#
# 記憶體不夠時 Pi 會不停換頁, 再加上 SD 卡讀取慢, 整台會卡住很久。
# 系統有 cgroup 記憶體控制時(開機參數 cgroup_enable=memory), 用 systemd-run 把編譯
# 關在 CNG6_BUILD_MEM(預設 1536M)裡: 超過時只有編譯失敗, 其他程式不受影響。

set -u
export PATH="$HOME/.cargo/bin:$PATH"
# 一次只編一個, 記憶體與 SD 卡的壓力最小
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
MEM="${CNG6_BUILD_MEM:-1536M}"
uid="$(id -u)"
# crontab 裡沒有這個變數, systemd-run --user 需要它
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$uid}"

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1
[ $# -eq 0 ] && set -- build
cmd=(nice -n 10 ionice -c3 cargo "$@")

controllers="/sys/fs/cgroup/user.slice/user-$uid.slice/user@$uid.service/cgroup.controllers"
if grep -qw memory "$controllers" 2>/dev/null &&
    systemd-run --user --scope -q true 2>/dev/null; then
    # MemoryHigh: 先慢下來(擠進 zram); MemoryMax: 真的超過就結束編譯
    exec systemd-run --user --scope -q --collect \
        -p MemoryHigh="$(( ${MEM%M} * 85 / 100 ))M" -p MemoryMax="$MEM" -p MemorySwapMax=1G \
        "${cmd[@]}"
fi
echo "[build] 注意: 沒有 cgroup 記憶體控制, 只用低優先權編譯" >&2
exec "${cmd[@]}"
