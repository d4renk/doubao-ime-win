# Doubao Voice Input - Portable Build Script
# 豆包语音输入便携版打包脚本

param(
    [switch]$Clean = $false,
    [string]$Version = "1.0.0"
)

$ErrorActionPreference = "Stop"
$RepoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))

Write-Host "🔧 Building Doubao Voice Input v$Version..." -ForegroundColor Cyan

# Build the canonical locked, static-CRT release.
Write-Host "🏗️ Building release version..." -ForegroundColor Yellow
& (Join-Path $PSScriptRoot "build-release.ps1") -Clean:$Clean

Push-Location $RepoRoot
try {

# Create portable directory
$PortableDir = "dist\doubao-voice-portable"
Write-Host "📁 Creating portable directory: $PortableDir" -ForegroundColor Yellow

if (Test-Path $PortableDir) {
    Remove-Item -Recurse -Force $PortableDir
}
New-Item -ItemType Directory -Force -Path $PortableDir | Out-Null

# Copy main executable
$ExePath = "target\x86_64-pc-windows-msvc\release\doubao-voice-input.exe"
if (Test-Path $ExePath) {
    Copy-Item $ExePath $PortableDir
    Write-Host "✅ Copied executable" -ForegroundColor Green
} else {
    Write-Host "❌ Executable not found: $ExePath" -ForegroundColor Red
    exit 1
}

# Copy configuration template
if (Test-Path "config.toml.example") {
    Copy-Item "config.toml.example" "$PortableDir\config.toml"
    Write-Host "✅ Copied configuration" -ForegroundColor Green
}

# Copy README
if (Test-Path "README.md") {
    Copy-Item "README.md" $PortableDir
    Write-Host "✅ Copied README" -ForegroundColor Green
}

# Create version file
"v$Version" | Out-File "$PortableDir\VERSION.txt" -Encoding UTF8

# Create ZIP archive
$ZipPath = "doubao-voice-input-v$Version-portable.zip"
Write-Host "📦 Creating ZIP archive: $ZipPath" -ForegroundColor Yellow

if (Test-Path $ZipPath) {
    Remove-Item $ZipPath
}
Compress-Archive -Path $PortableDir -DestinationPath $ZipPath -Force

# Get file size
$ExeSize = (Get-Item "$PortableDir\doubao-voice-input.exe").Length / 1MB
$ZipSize = (Get-Item $ZipPath).Length / 1MB

Write-Host ""
Write-Host "✅ Build completed successfully!" -ForegroundColor Green
Write-Host ""
Write-Host "📊 Build Statistics:" -ForegroundColor Cyan
Write-Host "   Executable size: $([math]::Round($ExeSize, 2)) MB"
Write-Host "   Archive size:    $([math]::Round($ZipSize, 2)) MB"
Write-Host ""
Write-Host "📁 Output files:" -ForegroundColor Cyan
Write-Host "   $PortableDir\"
Write-Host "   $ZipPath"
Write-Host ""
} finally {
    Pop-Location
}
