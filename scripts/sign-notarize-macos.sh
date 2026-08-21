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
NOTARY_RESULT=""

for var in APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do
  if [ -z "${!var:-}" ]; then
    echo "错误:环境变量 $var 未设置,无法签名公证" >&2
    exit 1
  fi
done
[ -d "$APP" ] || { echo "错误:$APP 不存在,请先运行 package-macos.sh" >&2; exit 1; }

# --- 导入证书到临时 keychain ---
echo "==> 创建临时 keychain 并导入证书..."
TMP_DIR="${RUNNER_TEMP:-$(mktemp -d)}"
KEYCHAIN="$TMP_DIR/signing.keychain-db"
KEYCHAIN_PWD="$(head -c 20 /dev/urandom | base64)"
CERT_P12="$TMP_DIR/certificate.p12"
# 结束时清理 keychain 与 p12(成功失败都清;runner 一次性,属双保险)
cleanup() {
  security delete-keychain "$KEYCHAIN" 2>/dev/null || true
  rm -f "$CERT_P12"
  [ -z "$NOTARY_RESULT" ] || rm -f "$NOTARY_RESULT"
}
trap cleanup EXIT
echo "$APPLE_CERTIFICATE" | base64 --decode -o "$CERT_P12"
security create-keychain -p "$KEYCHAIN_PWD" "$KEYCHAIN"
# 证书有效期外 keychain 自动锁定,避免残留
security set-keychain-settings -lut 21600 "$KEYCHAIN"
security unlock-keychain -p "$KEYCHAIN_PWD" "$KEYCHAIN"
security import "$CERT_P12" -P "$APPLE_CERTIFICATE_PASSWORD" -A -t cert -f pkcs12 -k "$KEYCHAIN"
# 把私钥访问分区授权给签名工具;必须带 codesign: 与 -s,否则无界面环境访问私钥会
# 卡在等待授权确认(实测挂死点),参考已验证可用的 Tauri 发布流程写法
echo "==> 设置 keychain 访问权限(apple-tool/codesign)..."
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PWD" "$KEYCHAIN"
security list-keychain -d user -s "$KEYCHAIN"

echo "==> 查找签名身份..."
# 原样打印身份列表,失败时日志能直接看出 keychain 里到底导入了什么
echo '$ security find-identity -v -p codesigning:'
security find-identity -v -p codesigning "$KEYCHAIN" || true
echo '$ security find-identity -p codesigning (含未受信链证书):'
security find-identity -p codesigning "$KEYCHAIN" || true
# 不用 -v:CI runner 上常缺中间证书导致链不受信,-v 会把身份过滤掉
IDENTITY="$(security find-identity -p codesigning "$KEYCHAIN" | grep 'Developer ID Application' | head -1 | sed -E 's/.*"(.*)".*/\1/' || true)"
[ -n "$IDENTITY" ] || { echo "错误:keychain 中未找到 Developer ID Application 证书;请对照上方列表检查(若两次都为空,说明 p12 里只有私钥没有证书,需从钥匙串访问导出整张证书重新生成 secret)" >&2; exit 1; }
echo "签名身份:$IDENTITY"

# --- 正式签名:hardened runtime + 安全时间戳,公证的硬性要求 ---
echo "==> 签名 .app(hardened runtime + timestamp)..."
codesign --force --deep --options runtime --timestamp --sign "$IDENTITY" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

# package-macos.sh 生成的 zip 仍包含 ad-hoc 签名版本。正式签名后必须先重打包，
# 否则本地验证的是新 app，Apple 收到的却是旧 zip，公证必然返回 Invalid。
echo "==> 重新打包正式签名后的 .app..."
rm -f "$ZIP"
# -y 保留 Frameworks 里的短名软链
(cd "$ROOT/dist" && zip -qry "transform-video-macos.zip" "TransformVideo.app")

# --- 公证:提交正式签名后的 zip,通过后 staple 回 .app 并最终重新打包 ---
echo "==> 提交公证(--wait 会等待 Apple 处理,几分钟到几十分钟属正常)..."
NOTARY_RESULT="$(mktemp "$TMP_DIR/notary-result.json.XXXXXX")"
xcrun notarytool submit "$ZIP" \
  --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" \
  --wait --output-format json | tee "$NOTARY_RESULT"

NOTARY_STATUS="$(plutil -extract status raw -o - "$NOTARY_RESULT")"
NOTARY_ID="$(plutil -extract id raw -o - "$NOTARY_RESULT")"
if [ "$NOTARY_STATUS" != "Accepted" ]; then
  echo "错误:Apple 公证失败(status=$NOTARY_STATUS, id=$NOTARY_ID),以下为公证日志:" >&2
  xcrun notarytool log "$NOTARY_ID" \
    --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" || true
  exit 1
fi

echo "==> staple 公证票据..."
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"
spctl --assess --type execute --verbose "$APP"

rm -f "$ZIP"
(cd "$ROOT/dist" && zip -qry "transform-video-macos.zip" "TransformVideo.app")
echo "签名公证完成:$ZIP"
