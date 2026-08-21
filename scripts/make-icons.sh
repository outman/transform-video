#!/usr/bin/env bash
# 从 assets/icon.png(方形设计稿,满铺背景,无需透明通道)生成两平台图标产物,均入库:
#   assets/AppIcon.icns  macOS .app 用(package-macos.sh 拷进 Contents/Resources)
#   assets/app.ico      Windows exe 资源用(build.rs 经 resources/windows/app.rc 嵌入)
# 分平台整形:macOS 按惯例套圆角透明遮罩,Windows 保持满铺方形。
# 换 logo 时:覆盖 assets/icon.png 后重跑本脚本,重新提交三个文件。
# 前置:brew install imagemagick;iconutil 为 macOS 自带。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/assets/icon.png"
ICONSET="$ROOT/assets/AppIcon.iconset"
TMP="$(mktemp -d)"
trap 'rm -rf "$ICONSET" "$TMP"' EXIT

[ -f "$SRC" ] || { echo "错误:缺少 $SRC(方形 PNG 设计稿)" >&2; exit 1; }
command -v magick >/dev/null || { echo "错误:需要 imagemagick(brew install imagemagick)" >&2; exit 1; }

# macOS 主图:缩到 1024 后套圆角遮罩(半径 ~18%),其余尺寸由它缩放。
# DstIn:保留底图在遮罩 alpha 内的部分(IM7 标准做法;勿用 -alpha off + CopyOpacity,
# 那是 IM6 配方,在 IM7 会把颜色变成纯灰)
magick "$SRC" -resize 1024x1024 -alpha set \
  \( -size 1024x1024 xc:none -draw 'roundrectangle 1,1 1022,1022 182,182' \) \
  -compose DstIn -composite "$TMP/mac-rounded.png"

# icns:iconset 目录的尺寸/命名是 iconutil 的固定约定
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  magick "$TMP/mac-rounded.png" -resize "${size}x${size}" "$ICONSET/icon_${size}x${size}.png"
  magick "$TMP/mac-rounded.png" -resize "$((size * 2))x$((size * 2))" "$ICONSET/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$ICONSET" -o "$ROOT/assets/AppIcon.icns"

# ico:一套多尺寸,Explorer/任务栏按需取
magick "$SRC" -define icon:auto-resize=256,128,64,48,32,16 "$ROOT/assets/app.ico"

echo "完成:assets/AppIcon.icns 与 assets/app.ico"
