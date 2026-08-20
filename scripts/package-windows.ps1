# 打包 win64:exe + vendor 的所有 dll 压成 zip。exe 同目录的 dll 会被 Windows 加载器优先找到。
$ErrorActionPreference = "Stop"
$ProgressPreference = 'SilentlyContinue'
$root = Split-Path -Parent $PSScriptRoot
$dist = Join-Path $root "dist/transform-video-win64"
Remove-Item -Recurse -Force $dist -ErrorAction Ignore
New-Item -ItemType Directory -Force -Path $dist | Out-Null
Copy-Item "$root/target/release/transform-video.exe" $dist
Copy-Item "$root/vendor/windows/bin/*.dll" $dist
if (Test-Path "$root/dist/transform-video-win64.zip") { Remove-Item "$root/dist/transform-video-win64.zip" }
Compress-Archive -Path $dist -DestinationPath "$root/dist/transform-video-win64.zip"
Write-Host "打包完成:$root/dist/transform-video-win64.zip"
