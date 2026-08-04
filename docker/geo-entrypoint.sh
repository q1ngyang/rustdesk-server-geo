#!/bin/sh
set -u

log() {
    printf '%s %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "[geoip] $*"
}

is_true() {
    case "$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')" in
        1|true|yes|y|on) return 0 ;;
        *) return 1 ;;
    esac
}

positive_integer_or_default() {
    value="$1"
    fallback="$2"
    case "$value" in
        ''|*[!0-9]*) printf '%s' "$fallback" ;;
        *) printf '%s' "$value" ;;
    esac
}

download_geoip() {
    url="${GEOIP_DB_URL:-}"
    target="${GEOIP_DB_PATH:-/root/geoip/GeoLite2-Country.mmdb}"
    timeout="$(positive_integer_or_default "${GEOIP_DOWNLOAD_TIMEOUT:-180}" 180)"
    minimum="$(positive_integer_or_default "${GEOIP_MIN_BYTES:-65536}" 65536)"

    if [ -z "$url" ]; then
        log "GEOIP_DB_URL 为空，跳过数据库下载"
        return 1
    fi

    target_dir="$(dirname "$target")"
    mkdir -p "$target_dir"
    tmp="${target}.download.$$"
    trap 'rm -f "$tmp"' INT TERM EXIT

    log "正在下载 GeoIP 数据库：$url"
    if ! curl -fL --retry 3 --retry-delay 3 --connect-timeout 20 \
        --max-time "$timeout" --output "$tmp" "$url"; then
        log "数据库下载失败；保留现有文件"
        rm -f "$tmp"
        trap - INT TERM EXIT
        return 1
    fi

    size="$(wc -c < "$tmp" | tr -d '[:space:]')"
    if [ "$size" -lt "$minimum" ]; then
        log "下载文件过小（${size} 字节，小于 ${minimum}）；拒绝替换"
        rm -f "$tmp"
        trap - INT TERM EXIT
        return 1
    fi

    if ! tail -c 131072 "$tmp" | grep -q 'MaxMind.com'; then
        log "下载文件缺少 MaxMind DB 标记；拒绝替换"
        rm -f "$tmp"
        trap - INT TERM EXIT
        return 1
    fi

    if [ -f "$target" ] && cmp -s "$tmp" "$target"; then
        log "GeoIP 数据库没有变化"
        rm -f "$tmp"
        trap - INT TERM EXIT
        return 2
    fi

    chmod 0644 "$tmp"
    mv -f "$tmp" "$target"
    trap - INT TERM EXIT
    log "GeoIP 数据库已原子更新：$target"
    return 0
}

reload_hbbs_geo() {
    if printf 'reload-geo\n' | nc -w 2 127.0.0.1 21115 >/dev/null 2>&1; then
        log "已通知 hbbs 重新加载 GeoIP 数据库和规则"
    else
        log "hbbs 尚未就绪，未执行热加载"
    fi
}

geoip_update_loop() {
    interval="$1"
    while sleep "$interval"; do
        if download_geoip; then
            reload_hbbs_geo
        fi
    done
}

if [ "${1:-}" = "hbbs" ]; then
    if is_true "${GEOIP_UPDATE_ON_START:-true}"; then
        download_geoip || true
    fi

    interval="$(positive_integer_or_default "${GEOIP_UPDATE_INTERVAL:-86400}" 86400)"
    if [ -n "${GEOIP_DB_URL:-}" ] && [ "$interval" -gt 0 ]; then
        geoip_update_loop "$interval" &
        log "GeoIP 自动更新已启用，周期为 ${interval} 秒"
    else
        log "GeoIP 周期更新已禁用"
    fi
fi

if [ "$#" -eq 0 ]; then
    log "缺少启动命令，请指定 hbbs、hbbr 或 rustdesk-utils"
    exit 64
fi

exec "$@"
