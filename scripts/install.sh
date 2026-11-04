#!/bin/bash

# 安装已有构建产物到 /Applications；替换过程不会停止正在运行的 CCProxy。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
exec bash "$SCRIPT_DIR/replace-app.sh" --skip-build

