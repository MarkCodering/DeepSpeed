# DeepSpeed installer for Windows
# Run from PowerShell: .\scripts\install.ps1
# Requires Rust (https://rustup.rs) and PowerShell 5+

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir  = Split-Path $MyInvocation.MyCommand.Path
$ProjectDir = Split-Path $ScriptDir
$BinDir     = "$env:LOCALAPPDATA\DeepSpeed"
$ConfigDir  = "$env:APPDATA\deepspeed"

Write-Host "=== DeepSpeed Installer (Windows) ===" -ForegroundColor Cyan
Write-Host ""

# 1. Build release binary
Write-Host "Building release binary..." -ForegroundColor Yellow
Set-Location $ProjectDir
cargo build --release
$Binary = "$ProjectDir\target\release\deepspeed.exe"
Write-Host "Binary: $Binary" -ForegroundColor Green
Write-Host ""

# 2. Install binary to %LOCALAPPDATA%\DeepSpeed
Write-Host "Installing to $BinDir\deepspeed.exe..."
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
Copy-Item $Binary "$BinDir\deepspeed.exe" -Force

# Add to user PATH if not already present
$CurrentPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($CurrentPath -notlike "*$BinDir*") {
    [Environment]::SetEnvironmentVariable("PATH", "$CurrentPath;$BinDir", "User")
    Write-Host "Added $BinDir to user PATH."
    Write-Host "Restart your terminal for 'deepspeed' to be available in PATH."
}
Write-Host ""

# 3. Install default config
if (-not (Test-Path "$ConfigDir\deepspeed.toml")) {
    Write-Host "Installing default config to $ConfigDir\..."
    New-Item -ItemType Directory -Force -Path $ConfigDir | Out-Null
    Copy-Item "$ProjectDir\config\deepspeed.toml" "$ConfigDir\deepspeed.toml"
    Write-Host ""
    Write-Host "IMPORTANT: Edit $ConfigDir\deepspeed.toml to set your Anthropic API key" -ForegroundColor Yellow
    Write-Host "  or set the environment variable: `$env:ANTHROPIC_API_KEY = 'sk-ant-...'"
    Write-Host ""
} else {
    Write-Host "Config already exists at $ConfigDir\deepspeed.toml — skipping."
}

# 4. Install Task Scheduler task
Write-Host "Installing Windows Task Scheduler task..."
& "$BinDir\deepspeed.exe" install
Write-Host ""

$LogPath = "$env:LOCALAPPDATA\DeepSpeed\deepspeed.log"
Write-Host "=== Installation complete ===" -ForegroundColor Green
Write-Host ""
Write-Host "Commands:"
Write-Host "  deepspeed status    — live system snapshot"
Write-Host "  deepspeed config    — show active configuration"
Write-Host "  deepspeed install   — reinstall Task Scheduler task"
Write-Host "  deepspeed uninstall — stop and remove task"
Write-Host ""
Write-Host "Log: $LogPath"
