# ytdlp-FFmpeg-GUI（影栈）本地构建脚本（PowerShell 7，与 CI 同构）
# 用法：pwsh ./scripts/build.ps1
# 前置：Windows 10/11 + WebView2 运行时（Win11 自带）+ Rust 工具链（rustup stable）
# 产物：target/release/ytdlp-FFmpeg-GUI.exe（workspace 根即仓库根，单 EXE 无需安装包）

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

Write-Host "==> [1/5] 核心层单元测试" -ForegroundColor Cyan
cargo test -p ytdlp-core
if ($LASTEXITCODE -ne 0) { throw "核心层测试失败" }

Write-Host "==> [2/5] Clippy：核心层（-D warnings 质量门禁）" -ForegroundColor Cyan
cargo clippy -p ytdlp-core --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { throw "Clippy 未通过（核心层）" }

# CI 的 build.yml 对两个 crate 都跑 Clippy，本地也必须跑同一个集合：
# 只跑核心层时，GUI crate 的 lint 要等推上去才暴露（历史上多次出现
# "本地全绿 → 推上去 CI 红 → 再补一个补丁提交"）。
Write-Host "==> [3/5] Clippy：GUI crate（-D warnings 质量门禁）" -ForegroundColor Cyan
cargo clippy -p ytdlp-gui --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { throw "Clippy 未通过（GUI crate）" }

Write-Host "==> [4/5] 依赖审计" -ForegroundColor Cyan
cargo audit 2>$null
if ($LASTEXITCODE -ne 0) { Write-Warning "cargo-audit 未安装或发现依赖漏洞（不影响本次构建，建议安装：cargo install cargo-audit）" }

Write-Host "==> [5/5] Release 构建" -ForegroundColor Cyan
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "Release 构建失败" }

$Exe = Join-Path $Root "target\release\ytdlp-FFmpeg-GUI.exe"
if (-not (Test-Path $Exe)) { throw "未找到产物：$Exe" }
$Size = (Get-Item $Exe).Length / 1MB
Write-Host "==> 构建完成：$Exe（$([math]::Round($Size,1)) MB）" -ForegroundColor Green
Write-Host "    交付形态：单 EXE（绿色便携，运行时目录 config/temp/tools 自动创建于 exe 同级）"
