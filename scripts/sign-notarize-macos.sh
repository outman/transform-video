#!/usr/bin/env bash
# 对 dist/TransformVideo.app 做 Developer ID 正式签名 + Apple 公证(notarization),
# 产物替换 dist/transform-video-macos.zip。前置:package-macos.sh 已跑(ad-hoc 会被
# 本脚本 --force 覆盖)。仅在 CI 发布链路使用,所需凭据由环境变量注入:
#   APPLE_CERTIFICATE            base64 编码的 .p12(Developer ID Application 证书)
#   APPLE_CERTIFICATE_PASSWORD   .p12 的导入密码
#   APPLE_ID                     Apple 账号邮箱
#   APPLE_PASSWORD               App 专用密码(notarytool 用,非账号密码)
#   APPLE_TEAM_ID                Team ID
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/dist/TransformVideo.app"
ZIP="$ROOT/dist/transform-video-macos.zip"

for var in APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do
  if [ -z "${!var:-}" ]; then
    echo "错误:环境变量 $var 未设置,无法签名公证" >&2
    exit 1
  fi
done
[ -d "$APP" ] || { echo "错误:$APP 不存在,请先运行 package-macos.sh" >&2; exit 1; }

# --- 导入证书到临时 keychain ---
TMP_DIR="${RUNNER_TEMP:-$(mktemp -d)}"
KEYCHAIN="$TMP_DIR/signing.keychain-db"
KEYCHAIN_PWD="$(head -c 20 /dev/urandom | base64)"
CERT_P12="$TMP_DIR/certificate.p12"
echo "$APPLE_CERTIFICATE" | base64 --decode -o "$CERT_P12"
security create-keychain -p "$KEYCHAIN_PWD" "$KEYCHAIN"
# 证书有效期外 keychain 自动锁定,避免残留
security set-keychain-settings -lut 21600 "$KEYCHAIN"
security unlock-keychain -p "$KEYCHAIN_PWD" "$KEYCHAIN"
security import "$CERT_P12" -P "$APPLE_CERTIFICATE_PASSWORD" -A -t cert -f pkcs12 -k "$KEYCHAIN"
# 允许 apple-tool(codesign)免交互使用私钥
security set-key-partition-list -S apple-tool:,apple: -k "$KEYCHAIN_PWD" "$KEYCHAIN"
security list-keychain -d user -s "$KEYCHAIN"

IDENTITY="$(security find-identity -v -p codesigning "$KEYCHAIN" | grep 'Developer ID Application' | head -1 | sed -E 's/.*"(.*)".*/\1/')"
[ -n "$IDENTITY" ] || { echo "错误:keychain 中未找到 Developer ID Application 证书(证书类型不对?)" >&2; exit 1; }
echo "签名身份:$IDENTITY"

# --- 正式签名:hardened runtime + 安全时间戳,公证的硬性要求 ---
codesign --force --deep --options runtime --timestamp --sign "$IDENTITY" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

# --- 公证:提交当前 zip,通过后 staple 回 .app 并重新打包 ---
xcrun notarytool submit "$ZIP" \
  --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" --wait
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"
spctl --assess --type execute --verbose "$APP"

rm -f "$ZIP"
# -y 保留 Frameworks 里的短名软链
(cd "$ROOT/dist" && zip -qry "transform-video-macos.zip" "TransformVideo.app")
echo "签名公证完成:$ZIP"
