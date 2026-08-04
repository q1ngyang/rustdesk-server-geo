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

duration_to_seconds() {
    raw="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
    fallback="$2"
    case "$raw" in
        *m) number="${raw%m}"; multiplier=60 ;;
        *h) number="${raw%h}"; multiplier=3600 ;;
        *d) number="${raw%d}"; multiplier=86400 ;;
        *s) number="${raw%s}"; multiplier=1 ;;
        *) number="$raw"; multiplier=1 ;;
    esac
    case "$number" in
        ''|*[!0-9]*) printf '%s' "$fallback" ;;
        *) printf '%s' "$((number * multiplier))" ;;
    esac
}

database_due() {
    target="$1"
    interval="$2"
    if [ ! -f "$target" ]; then
        return 0
    fi
    if [ "$interval" -le 0 ]; then
        return 1
    fi
    modified="$(stat -c '%Y' "$target" 2>/dev/null || printf '0')"
    now="$(date '+%s')"
    case "$modified" in
        ''|*[!0-9]*) return 0 ;;
    esac
    age="$((now - modified))"
    [ "$age" -ge "$interval" ]
}

download_database() {
    label="$1"
    url="$2"
    target="$3"
    timeout="$(positive_integer_or_default "${GEOIP_DOWNLOAD_TIMEOUT:-600}" 600)"
    minimum="$(positive_integer_or_default "${GEOIP_MIN_BYTES:-65536}" 65536)"

    target_dir="$(dirname "$target")"
    mkdir -p "$target_dir"
    tmp="${target}.download.$$"
    trap 'rm -f "$tmp"' INT TERM EXIT

    log "正在下载 ${label} 数据库：$url"
    if ! curl -fL --retry 3 --retry-delay 3 --connect-timeout 20 \
        --max-time "$timeout" --output "$tmp" "$url"; then
        log "${label} 数据库下载失败；保留现有文件"
        rm -f "$tmp"
        trap - INT TERM EXIT
        return 1
    fi

    size="$(wc -c < "$tmp" | tr -d '[:space:]')"
    if [ "$size" -lt "$minimum" ]; then
        log "${label} 下载文件过小（${size} 字节，小于 ${minimum}）；拒绝替换"
        rm -f "$tmp"
        trap - INT TERM EXIT
        return 1
    fi

    if ! tail -c 131072 "$tmp" | grep -q 'MaxMind.com'; then
        log "${label} 下载文件缺少 MaxMind DB 标记；拒绝替换"
        rm -f "$tmp"
        trap - INT TERM EXIT
        return 1
    fi

    if [ -f "$target" ] && cmp -s "$tmp" "$target"; then
        log "${label} 数据库没有变化"
        touch "$target"
        rm -f "$tmp"
        trap - INT TERM EXIT
        return 2
    fi

    chmod 0644 "$tmp"
    mv -f "$tmp" "$target"
    trap - INT TERM EXIT
    log "${label} 数据库已原子更新：$target"
    return 0
}

update_database_if_due() {
    label="$1"
    url="$2"
    target="$3"
    interval="$4"
    force="$5"

    if [ -z "$url" ]; then
        return 2
    fi
    if [ "$force" -ne 1 ] && ! database_due "$target" "$interval"; then
        return 2
    fi
    download_database "$label" "$url" "$target"
}

update_all_databases() {
    interval="$1"
    force="$2"
    updated=1

    country_url="${GEOIP_COUNTRY_DB_URL:-${GEOIP_DB_URL:-}}"
    country_path="${GEOIP_COUNTRY_DB_PATH:-${GEOIP_DB_PATH:-/root/geoip/GeoLite2-Country.mmdb}}"
    city_url="${GEOIP_CITY_DB_URL:-}"
    city_path="${GEOIP_CITY_DB_PATH:-/root/geoip/GeoLite2-City.mmdb}"
    asn_url="${GEOIP_ASN_DB_URL:-}"
    asn_path="${GEOIP_ASN_DB_PATH:-/root/geoip/GeoLite2-ASN.mmdb}"

    if update_database_if_due "Country" "$country_url" "$country_path" "$interval" "$force"; then
        updated=0
    fi
    if update_database_if_due "City" "$city_url" "$city_path" "$interval" "$force"; then
        updated=0
    fi
    if update_database_if_due "ASN" "$asn_url" "$asn_path" "$interval" "$force"; then
        updated=0
    fi
    return "$updated"
}

has_database_url() {
    [ -n "${GEOIP_COUNTRY_DB_URL:-${GEOIP_DB_URL:-}}" ] \
        || [ -n "${GEOIP_CITY_DB_URL:-}" ] \
        || [ -n "${GEOIP_ASN_DB_URL:-}" ]
}

reload_hbbs_geo() {
    if printf 'reload-geo\n' | nc -w 2 127.0.0.1 21115 >/dev/null 2>&1; then
        log "已通知 hbbs 重新加载 MMDB 和规则"
    else
        log "hbbs 尚未就绪，未执行热加载"
    fi
}

geoip_update_loop() {
    interval="$1"
    while sleep "$interval"; do
        if update_all_databases "$interval" 0; then
            reload_hbbs_geo
        fi
    done
}

if [ "${1:-}" = "hbbs" ]; then
    interval_raw="${GEOIP_UPDATE_INTERVAL:-168h}"
    interval="$(duration_to_seconds "$interval_raw" 604800)"

    if is_true "${GEOIP_UPDATE_ON_START:-true}"; then
        force=0
        if is_true "${GEOIP_FORCE_UPDATE_ON_START:-false}"; then
            force=1
        fi
        update_all_databases "$interval" "$force" || true
    fi

    if has_database_url && [ "$interval" -gt 0 ]; then
        geoip_update_loop "$interval" &
        log "MMDB 自动更新已启用，周期为 ${interval_raw}（${interval} 秒）"
    else
        log "MMDB 周期更新已禁用"
    fi
fi

if [ "$#" -eq 0 ]; then
    log "缺少启动命令，请指定 hbbs、hbbr 或 rustdesk-utils"
    exit 64
fi

exec "$@"
