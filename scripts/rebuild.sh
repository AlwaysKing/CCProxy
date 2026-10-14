#!/bin/bash

# 重新编译并打包 CCProxy 应用（Tauri）
# 使用方法: ./scripts/rebuild.sh [选项]
# 选项:
#   --skip-clean  跳过清理步骤
#   --only-build  只构建前端，不打包

set -e

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}═══════════════════════════════════════${NC}"
echo -e "${BLUE}   CCProxy 重新编译打包脚本${NC}"
echo -e "${BLUE}═══════════════════════════════════════${NC}"
echo ""

# 解析参数
SKIP_CLEAN=false
ONLY_BUILD=false

for arg in "$@"; do
    case $arg in
        --skip-clean)
            SKIP_CLEAN=true
            shift
            ;;
        --only-build)
            ONLY_BUILD=true
            shift
            ;;
        *)
            echo -e "${RED}未知参数: $arg${NC}"
            exit 1
            ;;
    esac
done

# 清理旧的构建文件
if [ "$SKIP_CLEAN" = false ]; then
    echo -e "${YELLOW}[1/3] 清理旧的构建文件...${NC}"
    rm -rf dist/
    rm -rf node_modules/.vite/
    rm -rf src-tauri/target/release/bundle/
    echo -e "${GREEN}✓ 清理完成${NC}"
    echo ""
else
    echo -e "${YELLOW}[1/3] 跳过清理步骤${NC}"
    echo ""
fi

# 只构建前端时到此结束
if [ "$ONLY_BUILD" = true ]; then
    echo -e "${YELLOW}[2/3] 构建前端资源...${NC}"
    npm run build
    echo ""
    echo -e "${GREEN}═══════════════════════════════════════${NC}"
    echo -e "${GREEN}   ✓ 前端构建完成（未打包应用）${NC}"
    echo -e "${GREEN}═══════════════════════════════════════${NC}"
    exit 0
fi

# 打包应用：tauri build 会先跑 beforeBuildCommand（npm run build）再编译 Rust 侧
echo -e "${YELLOW}[2/3] 构建前端 + 编译打包应用（Rust release，第一次会比较久）...${NC}"
npx tauri build --bundles app
echo ""

# 显示结果
echo -e "${YELLOW}[3/3] 打包完成${NC}"
echo ""
echo -e "${GREEN}═══════════════════════════════════════${NC}"
echo -e "${GREEN}   ✓ 重新编译打包完成！${NC}"
echo -e "${GREEN}═══════════════════════════════════════${NC}"
echo ""
echo -e "${BLUE}应用位置:${NC}"
ls -lh src-tauri/target/release/bundle/macos/CCProxy.app 2>/dev/null || echo "src-tauri/target/release/bundle/macos/"
echo ""
echo -e "${BLUE}提示: 运行 'open src-tauri/target/release/bundle/macos/CCProxy.app' 启动应用${NC}"

