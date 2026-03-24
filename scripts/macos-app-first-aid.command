#!/bin/bash
set -euo pipefail

APP_PATH="${1:-/Applications/OpenClaw 助手.app}"

echo "== OpenClaw 助手 macOS 修复 =="
echo "目标 App: $APP_PATH"
echo

if [ ! -e "$APP_PATH" ]; then
  echo "未找到 App：$APP_PATH"
  echo "请把实际 App 路径作为参数传入，例如："
  echo "  ./macos-app-first-aid.command \"/Applications/OpenClaw 助手.app\""
  exit 1
fi

echo "[1/2] 允许任何来源应用运行..."
sudo spctl --master-disable

echo
echo "[2/2] 移除应用的 quarantine 属性..."
sudo xattr -dr com.apple.quarantine "$APP_PATH"

echo
echo "已完成。现在请重新尝试打开：$APP_PATH"
read -r -p "按回车键退出..." _
