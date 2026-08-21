#!/usr/bin/env bash
# 组装 TransformVideo.app:vendor 构建的二进制 + vendor 真身 dylib + rpath + ad-hoc 签名。
# 前置:用 vendor/macos 作为 FFMPEG_DIR 的 release 构建(见 vendor-macos.sh 末尾打印的命令)。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/dist/TransformVideo.app"
rm -rf "$APP" "$ROOT/dist/transform-video-macos.zip"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks" "$APP/Contents/Resources"
cp "$ROOT/assets/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"

cp "$ROOT/target/release/transform-video" "$APP/Contents/MacOS/TransformVideo"
# vendor/macos/lib 里软链与真身并存:真身是全版本名文件,软链是短名别名。不能直接
# cp *.dylib——拷贝会解引用软链,产生重复大文件;真身用 cp,软链用 readlink+ln 重建。
# 两种名字都得进包:可执行文件引用全版本名,dylib 相互引用的是短名(如 @rpath/libavutil.61.dylib)。
for f in "$ROOT"/vendor/macos/lib/*.dylib; do
  if [ -L "$f" ]; then
    link_target="$(readlink "$f")"
    # vendor 产出应为相对软链;绝对软链进包后会指向构建机路径,直接报错
    case "$link_target" in
      /*) echo "错误:$f 是绝对软链($link_target),vendor 产物异常" >&2; exit 1 ;;
    esac
    ln -s "$link_target" "$APP/Contents/Frameworks/$(basename "$f")"
  else
    cp "$f" "$APP/Contents/Frameworks/"
  fi
done

EXE="$APP/Contents/MacOS/TransformVideo"
# 防呆:二进制若链接了系统 FFmpeg 的绝对路径(非 @rpath),打出的包在别的机器上必挂
if otool -L "$EXE" | grep -qE '^\s*/(opt/homebrew|usr/local)/'; then
  echo "错误:二进制链接了系统 FFmpeg(非 @rpath),请先用 vendor 构建:" >&2
  echo "  FFMPEG_DIR=\"$ROOT/vendor/macos\" RUSTFLAGS='-C link-arg=-Wl,-rpath,$ROOT/vendor/macos/lib' cargo build --release" >&2
  exit 1
fi
# 构建期 RUSTFLAGS 注入的 vendor 绝对路径 rpath 仅开发机有效,发布包里删掉,
# 只保留 @executable_path 相对 rpath,让 .app 完全自包含。
install_name_tool -delete_rpath "$ROOT/vendor/macos/lib" "$EXE" 2>/dev/null || true
if otool -l "$EXE" | grep -A2 LC_RPATH | grep -q '@executable_path/../Frameworks'; then
  :
else
  install_name_tool -add_rpath "@executable_path/../Frameworks" "$EXE"
fi

# Info.plist 版本跟随 Cargo.toml,避免发布包版本漂移
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -1)"
test -n "$VERSION" || { echo "错误:未能从 Cargo.toml 解析出版本号" >&2; exit 1; }

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key><string>TransformVideo</string>
	<key>CFBundleIdentifier</key><string>com.outman.transform-video</string>
	<key>CFBundleName</key><string>TransformVideo</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleIconFile</key><string>AppIcon</string>
	<key>CFBundleShortVersionString</key><string>$VERSION</string>
	<key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$APP"
# -y:保留 Frameworks 里的短名软链(不解引用成重复大文件)
(cd "$ROOT/dist" && zip -qry "transform-video-macos.zip" "TransformVideo.app")
echo "打包完成:$ROOT/dist/transform-video-macos.zip"
