#Requires -Version 5.1
<#
.SYNOPSIS
    Start Cteno desktop app (Windows)
.PARAMETER Mode
    Startup mode: 'tauri' (default, full app) or 'metro' (Metro bundler only)
#>
param(
    [ValidateSet('tauri', 'metro')]
    [string]$Mode = 'tauri'
)

$ErrorActionPreference = 'Stop'

# ── 确保常用工具在 PATH 中 ──
$extraPaths = @(
    (Join-Path $env:APPDATA 'npm'),           # yarn, npx
    (Join-Path $env:USERPROFILE '.cargo\bin')  # cargo, rustc
)
foreach ($p in $extraPaths) {
    if ((Test-Path $p) -and $env:Path -notlike "*$p*") {
        $env:Path = "$p;$env:Path"
    }
}

$RootDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$UnifiedSecretsFile = Join-Path $RootDir 'config\unified.secrets.json'
$SecretsProfile = if ($env:CTENO_SECRETS_PROFILE) { $env:CTENO_SECRETS_PROFILE } else { 'production' }

Write-Host "🚀 启动 Cteno (模式: $Mode)..."
$DevFrontendPort = 8081
$BackendPort = 19198

# ── 统一密钥同步 ──
if (Test-Path $UnifiedSecretsFile) {
    Write-Host "🔐 同步密钥 (profile: $SecretsProfile)..."
    $syncCmd = if ($SecretsProfile -eq 'dev') { 'secrets:sync:dev' } else { 'secrets:sync' }
    Push-Location $RootDir
    try {
        yarn $syncCmd 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Write-Host "❌ 密钥同步失败，请检查 config/unified.secrets.json"
            exit 1
        }
    } finally {
        Pop-Location
    }
}

# ── 停止旧进程 ──
Write-Host "🛑 停止旧进程..."
if ($Mode -eq 'tauri') {
    $procs = Get-Process -ErrorAction SilentlyContinue | Where-Object {
        $_.ProcessName -match 'cteno|tauri'
    }
    if ($procs) {
        $procs | Stop-Process -Force -ErrorAction SilentlyContinue
        Write-Host "  ✓ Tauri/Rust 进程已终止"
    } else {
        Write-Host "  - 无 Tauri 进程"
    }
}

# Kill Metro/Expo bundler on port 8081
try {
    $metroPids = Get-NetTCPConnection -LocalPort $DevFrontendPort -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty OwningProcess -Unique
    if ($metroPids) {
        $metroPids | ForEach-Object { Stop-Process -Id $_ -Force -ErrorAction SilentlyContinue }
        Write-Host "  ✓ Metro bundler ($DevFrontendPort) 已终止"
    } else {
        Write-Host "  - 无 Metro 进程"
    }
} catch {
    Write-Host "  - 无 Metro 进程"
}

Start-Sleep -Seconds 1

# ── 进入项目目录 ──
$DesktopDir = Join-Path $RootDir 'apps\client'

# ── 检查依赖 ──
if (-not (Test-Path (Join-Path $DesktopDir 'node_modules'))) {
    Write-Host "📦 首次启动，正在安装依赖..."
    Push-Location $DesktopDir
    try {
        yarn install
        if ($LASTEXITCODE -ne 0) {
            Write-Host "❌ 依赖安装失败"
            exit 1
        }
        Write-Host "✅ 依赖安装完成"
    } finally {
        Pop-Location
    }
}

$LogDir = $env:TEMP
$MetroLog = Join-Path $LogDir 'cteno-metro.log'
$MetroPidFile = Join-Path $LogDir 'cteno-metro.pid'
$CtenoLog = Join-Path $LogDir 'cteno.log'
$CtenoPidFile = Join-Path $LogDir 'cteno.pid'

if ($Mode -eq 'metro') {
    # ── 仅启动 Metro ──
    Write-Host "🎯 启动 Metro bundler (仅前端，用于调试)..."
    Write-Host "   - Metro: http://localhost:$DevFrontendPort"
    Write-Host ""

    Push-Location $DesktopDir
    $proc = Start-Process -FilePath 'npx' -ArgumentList "expo start --port $DevFrontendPort" `
        -RedirectStandardOutput $MetroLog -RedirectStandardError "$MetroLog.err" `
        -PassThru -WindowStyle Hidden
    Pop-Location

    $proc.Id | Out-File -FilePath $MetroPidFile -Encoding ascii

    Write-Host "⏳ 等待 Metro 启动..."
    Start-Sleep -Seconds 5

    Write-Host ""
    Write-Host "✅ Metro 启动完成！"
    Write-Host ""
    Write-Host "📝 日志："
    Write-Host "  - 实时日志: Get-Content -Wait $MetroLog"
    Write-Host "  - 完整日志: $MetroLog"
    Write-Host ""
    Write-Host "🛑 停止："
    Write-Host "  Stop-Process -Id (Get-Content $MetroPidFile)"
    Write-Host "  或: .\stop-cteno.ps1"

} else {
    # ── 完整启动: Tauri + Metro + 后端 ──
    Write-Host "🎯 启动 Tauri 开发服务器（包含前后端）..."
    Write-Host "   这会自动启动："
    Write-Host "   - Expo 前端服务器 (http://localhost:$DevFrontendPort)"
    Write-Host "   - Tauri 桌面应用窗口"
    Write-Host "   - 后端 API 服务器 (http://localhost:$BackendPort)"
    Write-Host ""

    $env:CTENO_DEV_FRONTEND_PORT = $DevFrontendPort

    Push-Location $DesktopDir
    $proc = Start-Process -FilePath 'yarn' -ArgumentList 'tauri:dev' `
        -RedirectStandardOutput $CtenoLog -RedirectStandardError "$CtenoLog.err" `
        -PassThru -WindowStyle Hidden
    Pop-Location

    $proc.Id | Out-File -FilePath $CtenoPidFile -Encoding ascii

    Write-Host "⏳ 等待服务启动..."
    Start-Sleep -Seconds 8

    Write-Host ""
    Write-Host "✅ Cteno 启动完成！"
    Write-Host ""
    Write-Host "📊 服务信息："
    Write-Host "  - 后端 API: http://localhost:$BackendPort"
    Write-Host "  - 前端开发: http://localhost:$DevFrontendPort"
    Write-Host "  - Tauri 窗口: 应该已打开"
    Write-Host ""
    Write-Host "📝 日志："
    Write-Host "  - 实时日志: Get-Content -Wait $CtenoLog"
    Write-Host "  - 完整日志: $CtenoLog"
    Write-Host ""
    Write-Host "🛑 停止："
    Write-Host "  .\stop-cteno.ps1"
}
