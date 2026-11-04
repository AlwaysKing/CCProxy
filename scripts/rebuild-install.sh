#!/bin/bash

# 重新编译并原子替换 /Applications/CCProxy.app。
# 不停止当前进程；兼容原来的 --skip-clean 参数。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

args=()
for arg in "$@"; do
    case "$arg" in
        --skip-clean)
            ;;
        --only-build)
            echo "rebuild-install.sh 不再支持 --only-build；请直接运行 ./scripts/rebuild.sh --only-build" >&2
            exit 1
            ;;
        *)
            args+=("$arg")
            ;;
    esac
done

exec bash "$SCRIPT_DIR/replace-app.sh" "${args[@]}"
