#!/usr/bin/env bash
# 下载 BtbN 的 win64 GPL shared 构建到 vendor/windows(CI 里 vendor-windows.ps1 的 bash 等价版,
# 供 macOS 交叉编译或 Linux 使用)。
# 优先取与 ffmpeg-next 主版本(9)对齐的 n9.x 线;没有则回退 master-latest。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENDOR="$ROOT/vendor/windows"

json="$(curl -fsSL https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/tags/latest)"

url="$(printf '%s' "$json" | grep -oE 'https://[^"]*ffmpeg-n9\.[^"]*-win64-gpl-shared-9\.[0-9]+\.zip' | head -n 1 || true)"
if [ -z "$url" ]; then
  echo "未找到 n9.x 资产,回退 master 构建,可能与 ffmpeg-next 9 的 ABI 不匹配" >&2
  url="$(printf '%s' "$json" | grep -oE 'https://[^"]*ffmpeg-master-latest-win64-gpl-shared\.zip' | head -n 1 || true)"
fi
[ -n "$url" ] || { echo "错误:BtbN latest release 中找不到合适的 win64 gpl shared 资产" >&2; exit 1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "下载:$url"
curl -fL --retry 3 -o "$tmp/ffmpeg.zip" "$url"
unzip -q "$tmp/ffmpeg.zip" -d "$tmp/un"
src="$(ls -d "$tmp"/un/*/ | head -n 1)"

rm -rf "$VENDOR"
mkdir -p "$VENDOR"
cp -R "$src/bin" "$src/lib" "$src/include" "$VENDOR/"

echo "完成:$VENDOR"
