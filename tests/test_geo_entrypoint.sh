#!/bin/sh
set -eu

repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
test_root="$(mktemp -d)"

cleanup() {
    rm -rf "$test_root"
}

fail() {
    printf '测试失败：%s\n' "$1" >&2
    cleanup
    exit 1
}

export GEOIP_ENTRYPOINT_SOURCE_ONLY=true
# shellcheck source=../docker/geo-entrypoint.sh
. "$repo_root/docker/geo-entrypoint.sh"

valid_source="$test_root/valid.mmdb"
invalid_source="$test_root/no-marker.mmdb"

# 模拟真实 MMDB：主体含大量 NUL，标准元数据魔数位于文件末尾。
dd if=/dev/zero of="$valid_source" bs=1024 count=70 2>/dev/null
printf '\253\315\357MaxMind.com' >> "$valid_source"
dd if=/dev/zero of="$invalid_source" bs=1024 count=70 2>/dev/null

has_mmdb_marker "$valid_source" || fail "没有识别有效 MMDB 魔数"

# 相同体积但没有末尾魔数时必须拒绝。
if has_mmdb_marker "$invalid_source"; then
    fail "错误接受了缺少 MMDB 魔数的文件"
fi

cleanup
printf 'MMDB 入口脚本测试通过\n'
