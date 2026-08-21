#!/usr/bin/env bash
# 打包 win64:exe + vendor 的所有 dll 压成 zip(CI 里 package-windows.ps1 的 bash 等价版,
# 读取交叉编译产物 target/<triple>/release/)。exe 同目录的 dll 会被 Windows 加载器优先找到。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${TARGET:-x86_64-pc-windows-msvc}"
EXE="$ROOT/target/$TARGET/release/transform-video.exe"
DIST="$ROOT/dist/transform-video-win64"

[ -f "$EXE" ] || { echo "错误:找不到 $EXE,请先运行 scripts/build-windows-macos.sh" >&2; exit 1; }

rm -rf "$DIST" "$ROOT/dist/transform-video-win64.zip"
mkdir -p "$DIST"
cp "$EXE" "$DIST/"
cp "$ROOT"/vendor/windows/bin/*.dll "$DIST/"
(cd "$ROOT/dist" && zip -qry transform-video-win64.zip transform-video-win64)
echo "打包完成:$ROOT/dist/transform-video-win64.zip"
