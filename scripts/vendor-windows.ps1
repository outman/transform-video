# 下载 BtbN 的 win64 GPL shared 构建到 vendor/windows。
# 优先取与 ffmpeg-next 主版本(9)对齐的 n9.x 线;没有则回退 master-latest。
$ErrorActionPreference = "Stop"
# PS 5.1:禁进度条(大文件下载快一个数量级),Invoke-WebRequest 走基础解析
$ProgressPreference = 'SilentlyContinue'

# Windows PowerShell 5.1 需要;pwsh 7 上无害
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$root = Split-Path -Parent $PSScriptRoot
$vendor = Join-Path $root "vendor/windows"

$release = Invoke-RestMethod "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/tags/latest"
$asset = $release.assets | Where-Object { $_.name -match "^ffmpeg-n9\..*-win64-gpl-shared-9\.\d+\.zip$" } | Select-Object -First 1
if (-not $asset) {
  Write-Warning "未找到 n9.x 资产,回退 master 构建,可能与 ffmpeg-next 9 的 ABI 不匹配"
  $asset = $release.assets | Where-Object { $_.name -eq "ffmpeg-master-latest-win64-gpl-shared.zip" } | Select-Object -First 1
}
if (-not $asset) { throw "BtbN latest release 中找不到合适的 win64 gpl shared 资产" }
$url = $asset.browser_download_url
Write-Host "下载:$url"

$zip = Join-Path $env:TEMP "ffmpeg-win64-gpl-shared.zip"
$un = Join-Path $env:TEMP "ffmpeg-win64-gpl-shared"
Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
Remove-Item -Recurse -Force $un -ErrorAction Ignore
Expand-Archive -Path $zip -DestinationPath $un -Force
$src = (Get-ChildItem $un -Directory | Select-Object -First 1).FullName

Remove-Item -Recurse -Force $vendor -ErrorAction Ignore
New-Item -ItemType Directory -Force -Path $vendor | Out-Null
Copy-Item -Recurse (Join-Path $src "bin") (Join-Path $vendor "bin")
Copy-Item -Recurse (Join-Path $src "lib") (Join-Path $vendor "lib")
Copy-Item -Recurse (Join-Path $src "include") (Join-Path $vendor "include")

Write-Host "完成:$vendor(资产:$($asset.name))"
Write-Host "构建:`$env:FFMPEG_DIR = `"$vendor`"; cargo build --release"
