param(
    [switch]$Clean = $false
)

$ErrorActionPreference = "Stop"

$Target = "x86_64-pc-windows-msvc"
$StaticCrtRustFlag = "-C target-feature=+crt-static"
$RepoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$ReleaseDirectory = Join-Path $RepoRoot "target\$Target\release"
$HadRustFlags = Test-Path Env:RUSTFLAGS
$PreviousRustFlags = $env:RUSTFLAGS

try {
    $env:RUSTFLAGS = if ([string]::IsNullOrWhiteSpace($PreviousRustFlags)) {
        $StaticCrtRustFlag
    } else {
        "$PreviousRustFlags $StaticCrtRustFlag"
    }

    if ($Clean -and (Test-Path -LiteralPath $ReleaseDirectory)) {
        Write-Host "Cleaning $ReleaseDirectory" -ForegroundColor Yellow
        Remove-Item -LiteralPath $ReleaseDirectory -Recurse -Force
    }

    Push-Location $RepoRoot
    try {
        Write-Host "Building locked static-CRT release for $Target" -ForegroundColor Cyan
        & cargo build --locked --release --target $Target
        if ($LASTEXITCODE -ne 0) {
            throw "Cargo release build failed with exit code $LASTEXITCODE."
        }
    } finally {
        Pop-Location
    }
} finally {
    if ($HadRustFlags) {
        $env:RUSTFLAGS = $PreviousRustFlags
    } else {
        Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
    }
}
