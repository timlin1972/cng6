#!/bin/bash
# 續約 Tailscale 的 HTTPS 憑證(Let's Encrypt, 90 天有效), 給 crontab 每月執行:
#
#   17 4 1 * * /home/timlin/cng6/utils/tls-renew.sh >> $HOME/.cache/cng6/tls-renew.log 2>&1
#
#   utils/tls-renew.sh [domain]    domain 預設用 tailscale 給這台機器的名稱 (xxx.ts.net)
#
# 憑證寫到 cng6 目錄的 <domain>.crt / <domain>.key (web.cert / web.key 指向這兩個檔案)。
# 剩不到 45 天才會真的換新, 所以每月跑一次一定來得及; 檔案換了以後,
# 執行中的 web 一分鐘內會自動改用新的, 不用重新啟動。
# tailscale cert 要 root 權限(sudo 不能問密碼), 產生的檔案再改回目前的使用者。

set -euo pipefail
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:$PATH"
dir="$(cd "$(dirname "$0")/.." && pwd)"
domain="${1:-$(tailscale status --json | python3 -c 'import json,sys; print((json.load(sys.stdin).get("CertDomains") or [""])[0])')}"
if [ -z "$domain" ]; then
    echo "$(date '+%F %T') 找不到 Tailscale 憑證名稱 (管理後台要打開 MagicDNS 與 HTTPS Certificates)" >&2
    exit 1
fi

# 和憑證放在同一個檔案系統, 最後的 mv 才是原子的
tmp="$(mktemp -d "$dir/.tls-renew.XXXXXX")"
trap 'sudo -n rm -rf "$tmp"' EXIT
sudo -n tailscale cert --min-validity 1080h \
    --cert-file "$tmp/crt" --key-file "$tmp/key" "$domain" >/dev/null
sudo -n chown "$(id -u):$(id -g)" "$tmp/crt" "$tmp/key"
chmod 644 "$tmp/crt"
chmod 600 "$tmp/key"

crt="$dir/$domain.crt"
key="$dir/$domain.key"
if cmp -s "$tmp/crt" "$crt" && cmp -s "$tmp/key" "$key"; then
    echo "$(date '+%F %T') $domain 憑證不用換, 到期: $(openssl x509 -in "$crt" -noout -enddate | cut -d= -f2)"
    exit 0
fi
# 同一個檔案系統內 mv 是原子的; 先換私鑰再換憑證, web 讀到一半時會先用舊的, 下次檢查再換
mv -f "$tmp/key" "$key"
mv -f "$tmp/crt" "$crt"
echo "$(date '+%F %T') $domain 憑證已更新, 到期: $(openssl x509 -in "$crt" -noout -enddate | cut -d= -f2)"
