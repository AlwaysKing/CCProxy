#!/bin/bash

# 构建并原子替换 /Applications/CCProxy.app。
# 不停止正在运行的进程：旧进程继续使用已加载的代码，下一次启动才切到新版本。
#
# 用法:
#   ./scripts/replace-app.sh                构建并替换
#   ./scripts/replace-app.sh --skip-build   只替换现有构建产物
#   ./scripts/replace-app.sh --dry-run      只检查产物和路径，不写 /Applications
#   ./scripts/replace-app.sh --clean        构建前清理 release bundle

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SOURCE_APP="$ROOT_DIR/src-tauri/target/release/bundle/macos/CCProxy.app"
TARGET_APP="/Applications/CCProxy.app"
STAGING_APP="/Applications/.CCProxy.app.staging.$$"
BACKUP_APP="/Applications/.CCProxy.app.backup.$$"

SKIP_BUILD=false
DRY_RUN=false
CLEAN_BUILD=false

for arg in "$@"; do
    case "$arg" in
        --skip-build)
            SKIP_BUILD=true
            ;;
        --dry-run)
            DRY_RUN=true
            ;;
        --clean)
            CLEAN_BUILD=true
            ;;
        -h|--help)
            sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo -e "${RED}未知参数: $arg${NC}" >&2
            exit 1
            ;;
    esac
done

cleanup() {
    if [ -d "$STAGING_APP" ]; then
        rm -rf "$STAGING_APP"
    fi
    if [ -d "$BACKUP_APP" ] && [ ! -d "$TARGET_APP" ]; then
        mv "$BACKUP_APP" "$TARGET_APP"
    fi
}
trap cleanup EXIT

echo -e "${BLUE}═══════════════════════════════════════${NC}"
echo -e "${BLUE}   CCProxy 原子替换${NC}"
echo -e "${BLUE}═══════════════════════════════════════${NC}"
echo ""

if [ "$SKIP_BUILD" = false ]; then
    build_args=(--skip-clean)
    if [ "$CLEAN_BUILD" = true ]; then
        build_args=()
    fi
    echo -e "${YELLOW}[1/4] 构建 release 应用...${NC}"
    bash "$ROOT_DIR/scripts/rebuild.sh" "${build_args[@]}"
    echo ""
else
    echo -e "${YELLOW}[1/4] 跳过构建${NC}"
    echo ""
fi

if [ ! -d "$SOURCE_APP" ]; then
    echo -e "${RED}✗ 找不到构建产物: $SOURCE_APP${NC}" >&2
    exit 1
fi
if [ ! -x "$SOURCE_APP/Contents/MacOS/ccproxy" ]; then
    echo -e "${RED}✗ 构建产物里没有可执行文件${NC}" >&2
    exit 1
fi

echo -e "${YELLOW}[2/4] 准备暂存目录...${NC}"
ditto "$SOURCE_APP" "$STAGING_APP"
if ! codesign --verify --deep --strict "$STAGING_APP" >/dev/null 2>&1; then
    echo -e "${RED}✗ 暂存应用的代码签名校验失败${NC}" >&2
    exit 1
fi
echo -e "${GREEN}✓ 暂存应用校验通过${NC}"
echo ""

if [ "$DRY_RUN" = true ]; then
    echo -e "${YELLOW}[3/4] dry-run：跳过 /Applications 替换${NC}"
    echo -e "${YELLOW}[4/4] 目标应用未修改${NC}"
    exit 0
fi

echo -e "${YELLOW}[3/4] 原子替换应用目录...${NC}"
if [ -d "$TARGET_APP" ]; then
    mv "$TARGET_APP" "$BACKUP_APP"
fi
if ! mv "$STAGING_APP" "$TARGET_APP"; then
    if [ -d "$BACKUP_APP" ]; then
        mv "$BACKUP_APP" "$TARGET_APP"
    fi
    echo -e "${RED}✗ 替换失败，已恢复旧版本${NC}" >&2
    exit 1
fi
rm -rf "$BACKUP_APP"
echo -e "${GREEN}✓ 已替换 $TARGET_APP${NC}"
echo ""

running_pids="$(pgrep -f "$TARGET_APP/Contents/MacOS/ccproxy" || true)"
echo -e "${YELLOW}[4/4] 完成${NC}"
if [ -n "$running_pids" ]; then
    echo -e "${GREEN}当前进程未被停止，连接保持:${NC} $(echo "$running_pids" | tr '\n' ' ')"
    echo -e "${BLUE}旧进程继续运行；等你主动退出并重新打开 CCProxy 后，新版本生效。${NC}"
else
    echo -e "${BLUE}当前没有运行中的 CCProxy；下次打开时会使用新版本。${NC}"
fi
