#!/usr/bin/env bash
# 在 macOS 上交叉编译 Windows 版并打包,产物 dist/transform-video-win64.zip(与 CI 的
# windows job 同形态)。target 用 x86_64-pc-windows-msvc,与 CI 同 ABI,ffmpeg vendor 同款。
#
# 工具链分工:
#   llvm(brew)     提供 clang-cl(交叉 C 编译器)与 llvm-rc(gpui manifest 的 rc 编译);
#                  不在默认 PATH,按 arm/x86 前缀找。lld-link 无需安装——cargo-xwin
#                  会把 rustup 自带的 rust-lld 软链成 lld-link 用
#   cargo-xwin     下载 MSVC CRT + Windows SDK 库,并给 cargo 注入交叉环境
#   rustup target  x86_64-pc-windows-msvc 的标准库(脚本自行安装)
#
# 前置:
#   brew install llvm
#   vendor/windows/shaders_bytes.rs —— gpui 的 DXBC 着色器预编译产物(zed 上游只在
#   Windows 宿主上做这步)。在任意 Windows 机器(含 CI 的 windows job)对同一 Cargo.lock
#   执行 cargo build --release 后,从 target/release/build/gpui_windows-*/out/ 拷出。
#   zed 依赖版本(Cargo.lock)变化时需重新生成。
# cargo-xwin 首次运行会从微软服务器下载 SDK(几百 MB,有本地缓存)。
# 注意:交叉产物无法在本机运行,测试仍走 CI 的 windows runner。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="x86_64-pc-windows-msvc"

# brew llvm 不在默认 PATH 里;按 arm/x86 前缀找
LLVM_BIN=""
for d in /opt/homebrew/opt/llvm/bin /usr/local/opt/llvm/bin; do
  if [ -x "$d/clang-cl" ] && [ -x "$d/llvm-rc" ]; then LLVM_BIN="$d"; break; fi
done
if [ -z "$LLVM_BIN" ] && command -v clang-cl >/dev/null 2>&1; then
  LLVM_BIN="$(dirname "$(command -v clang-cl)")"
fi
if [ -z "$LLVM_BIN" ]; then
  echo "错误:找不到 llvm(clang-cl/llvm-rc),请先:brew install llvm" >&2
  exit 1
fi
export PATH="$LLVM_BIN:$PATH"
# ffmpeg-sys-next 的 bindgen 要加载 libclang
export LIBCLANG_PATH="${LIBCLANG_PATH:-"$(dirname "$LLVM_BIN")/lib"}"

# 防 macOS 本地构建遗留的 RUSTFLAGS(rpath 等参数会干扰 lld-link)
unset RUSTFLAGS || true
# gpui 的 .rc 资源走 llvm-rc,需要 wrapper 修正工作目录(见 xwin-llvm-rc.sh 头注释)
export RC="$ROOT/scripts/xwin-llvm-rc.sh"

rustup target add "$TARGET"
command -v cargo-xwin >/dev/null 2>&1 || cargo install cargo-xwin --locked

# vendor 幂等:已有则复用,REFRESH_VENDOR=1 强制重下
if [ ! -d "$ROOT/vendor/windows/lib" ] || [ "${REFRESH_VENDOR:-0}" = "1" ]; then
  "$ROOT/scripts/vendor-windows.sh"
fi

# gpui_windows 的 build script 只在宿主机是 Windows 时才用 fxc.exe 预编译 HLSL 着色器
# (target_os cfg 的是宿主而非目标),交叉时为空跑,release 代码 include! 的 shaders_bytes.rs
# 会缺失。该文件是纯构建产物:在任意 Windows 机器上 cargo build --release 后,从
# target/release/build/gpui_windows-*/out/ 拷出,放到 vendor/windows/shaders_bytes.rs。
# 这里在两遍构建之间把它注进 OUT_DIR——首跑让空跑的 build script 建出 OUT_DIR,
# 注入后复跑(其 rerun-if-changed 只盯 .hlsl,不会删掉注入的文件)。
SHADERS="$ROOT/vendor/windows/shaders_bytes.rs"
if [ -f "$SHADERS" ]; then
  shopt -s nullglob
  outs=("$ROOT"/target/$TARGET/release/build/gpui_windows-*/out)
  inject=0
  [ ${#outs[@]} -eq 0 ] && inject=1
  # bash 3.2(set -u)下空数组要带守卫展开
  for out in ${outs[@]+"${outs[@]}"}; do
    if [ ! -f "$out/shaders_bytes.rs" ] || ! cmp -s "$SHADERS" "$out/shaders_bytes.rs"; then
      inject=1
    fi
  done
  # 已注入过但内容变了(zed rev 更新):cargo 感知不到该文件变化,必须清缓存防陈旧产物
  for out in ${outs[@]+"${outs[@]}"}; do
    if [ -f "$out/shaders_bytes.rs" ] && ! cmp -s "$SHADERS" "$out/shaders_bytes.rs"; then
      echo "shaders_bytes.rs 已更新,清空交叉编译缓存重建:$ROOT/target/$TARGET"
      rm -rf "$ROOT/target/$TARGET"
      inject=1
      break
    fi
  done
  if [ "$inject" = 1 ]; then
    FFMPEG_DIR="$ROOT/vendor/windows" cargo xwin build --release --target "$TARGET" || true
    for out in "$ROOT"/target/$TARGET/release/build/gpui_windows-*/out; do
      [ -d "$out" ] && cp -f "$SHADERS" "$out/shaders_bytes.rs"
    done
  fi
else
  echo "提示:缺 vendor/windows/shaders_bytes.rs(gpui 的 DXBC 着色器预编译产物)," >&2
  echo "      release 交叉构建会在 gpui_windows 处失败。获取方式见脚本头注释。" >&2
fi

FFMPEG_DIR="$ROOT/vendor/windows" cargo xwin build --release --target "$TARGET"

"$ROOT/scripts/package-windows.sh"
