#!/usr/bin/env bash
# llvm-rc 包装,仅交叉编译 Windows 时由 RC 环境变量指定(embed-resource 会优先用它)。
#
# 缘由:embed-resource 3.0.11 在非 Windows 宿主上把 llvm-rc 的工作目录切到 .rc 所在目录,
# 而 gpui 的 .rc 里引用的 manifest 路径是相对 crate 根的,于是报 file not found
# (Windows 宿主不改 cwd,build script 的 cwd 就是 crate 根,所以 CI 上没有此问题)。
#
# 做法:取末位参数(preprocessed .rc,绝对路径),从中读出第一个带 / 的引号路径,
# 从当前目录向上回溯直到该相对路径可解析,cd 过去再转发真正的 llvm-rc。
# llvm-rc 的输入与 /fo 输出都是绝对路径,换 cwd 不影响。
set -euo pipefail

REAL="$(command -v llvm-rc)" || { echo "错误:PATH 中没有 llvm-rc(brew install llvm)" >&2; exit 1; }

rcfile=""
if [ $# -gt 0 ]; then rcfile="${@: -1}"; fi
if [ -f "$rcfile" ]; then
  rel="$(grep -oE '"[^"]+"' "$rcfile" | tr -d '"' | grep / | head -n 1 || true)"
  if [ -n "$rel" ]; then
    dir=.
    while :; do
      if [ -e "$dir/$rel" ]; then cd "$dir"; break; fi
      [ "$(cd "$dir" && pwd)" = / ] && break
      dir="$dir/.."
    done
  fi
fi
exec "$REAL" "$@"
