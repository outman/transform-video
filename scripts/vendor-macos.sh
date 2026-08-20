#!/usr/bin/env bash
# 从 Homebrew 提取 FFmpeg 动态库到 vendor/macos/,install_name 全部改 @rpath。
# 前置:brew install ffmpeg
#
# 注意 brew 的目录布局:真实文件是带完整版本号的(如 libavcodec.63.1.100.dylib),
# 而 install name / 相互引用用的是短名(如 libavcodec.63.dylib),未版本号名
# (libavcodec.dylib)只是软链。因此拷贝时:真实文件入库 + 按引用名补软链,
# 链接用的 -lavcodec 依赖未版本号别名存在。
set -euo pipefail

FF="$(brew --prefix ffmpeg)"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENDOR="$ROOT/vendor/macos"
rm -rf "$VENDOR"
mkdir -p "$VENDOR/lib" "$VENDOR/include" "$VENDOR/lib/pkgconfig"

# otool -L 里属于 Homebrew 前缀的依赖路径(含 Cellar 与 opt 两种写法)
brew_dep() { otool -L "$1" | tail -n +2 | awk '{print $1}' | grep -E '^(/opt/homebrew|/usr/local)/' || true; }

# 拷贝一个 brew 依赖:真实文件进 vendor;若被引用名与真实文件名不同,补同名软链。
# 幂等;新增文件或软链时置 ADDED=1 供闭包循环判断。
ADDED=0
vendor_copy() {
  local src="$1" base real realbase
  base="$(basename "$src")"
  real="$(readlink -f "$src")"
  realbase="$(basename "$real")"
  if [ ! -e "$VENDOR/lib/$realbase" ]; then
    cp "$real" "$VENDOR/lib/"
    chmod u+w "$VENDOR/lib/$realbase"
    ADDED=1
  fi
  if [ "$base" != "$realbase" ] && [ ! -e "$VENDOR/lib/$base" ]; then
    ln -s "$realbase" "$VENDOR/lib/$base"
    ADDED=1
  fi
}

# 直接需要的库(av* + sw*;ffmpeg-sys-next 的 avdevice 特性默认开启,链接期要找
# libavdevice.dylib。libx264 及 aom/dav1d 等传递依赖由闭包拷贝带入)
for lib in libavcodec libavdevice libavformat libavfilter libavutil libswresample libswscale; do
  for dylib in "$FF"/lib/${lib}.*.dylib; do
    vendor_copy "$dylib"
    break
  done
done

# 闭包拷贝:brew 前缀下的传递依赖(GPL 构建会带 aom/dav1d/x265/openssl 等)
changed=1
while [ "$changed" -eq 1 ]; do
  changed=0
  for f in "$VENDOR/lib"/*.dylib; do
    if [ -L "$f" ]; then continue; fi
    while IFS= read -r dep; do
      ADDED=0
      vendor_copy "$dep"
      if [ "$ADDED" -eq 1 ]; then changed=1; fi
    done < <(brew_dep "$f")
  done
done

# cargo:rustc-link-lib=avcodec → 链接器找 libavcodec.dylib,补未版本号别名。
# 须在闭包拷贝之后:libx264 等由闭包带入。
for lib in libavcodec libavdevice libavformat libavfilter libavutil libswresample libswscale libx264; do
  if [ ! -e "$VENDOR/lib/$lib.dylib" ]; then
    for target in "$VENDOR"/lib/${lib}.*.dylib; do
      ln -s "$(basename "$target")" "$VENDOR/lib/$lib.dylib"
      break
    done
  fi
done

# 改 id 与引用为 @rpath,重签名(arm 上改完必须 ad-hoc 重签,否则 dyld 拒载)
for f in "$VENDOR/lib"/*.dylib; do
  if [ -L "$f" ]; then continue; fi
  base="$(basename "$f")"
  install_name_tool -id "@rpath/$base" "$f"
  while IFS= read -r dep; do
    depbase="$(basename "$dep")"
    if [ ! -e "$VENDOR/lib/$depbase" ]; then
      echo "错误:依赖未入库,无法改写:$f -> $dep" >&2
      exit 1
    fi
    install_name_tool -change "$dep" "@rpath/$depbase" "$f"
  done < <(brew_dep "$f")
  codesign --force --sign - "$f"
done

# 校验:改完后所有依赖应只剩 @rpath、/usr/lib 或 /System
for f in "$VENDOR/lib"/*.dylib; do
  if [ -L "$f" ]; then continue; fi
  while IFS= read -r dep; do
    echo "错误:残留外部引用 $f -> $dep" >&2
    exit 1
  done < <(otool -L "$f" | tail -n +2 | awk '{print $1}' | grep -vE '^(@rpath/|/usr/lib/|/System/)' || true)
done

# 头文件与 .pc(开发期 FFMPEG_DIR 构建用;FFMPEG_DIR 生效时不读 .pc)
cp -R "$FF/include/"* "$VENDOR/include/"
cp "$FF"/lib/pkgconfig/*.pc "$VENDOR/lib/pkgconfig/"

echo "完成:$VENDOR"
echo "构建(带 rpath 让产物直接可运行):"
echo "  FFMPEG_DIR=$VENDOR RUSTFLAGS='-C link-arg=-Wl,-rpath,$VENDOR/lib' cargo build --release"
echo "  FFMPEG_DIR=$VENDOR RUSTFLAGS='-C link-arg=-Wl,-rpath,$VENDOR/lib' cargo test --all"
