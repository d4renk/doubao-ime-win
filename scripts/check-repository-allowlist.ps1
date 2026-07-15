$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$allowedPatterns = @(
    '^\.gitignore$',
    '^\.github/.+\.(?:yml|yaml)$',
    '^Cargo\.(?:lock|toml)$',
    '^README\.md$',
    '^SECURITY\.md$',
    '^build\.rs$',
    '^config\.toml$',
    '^config\.toml\.example$',
    '^rust-toolchain\.toml$',
    '^protoc-33\.4-win64\.zip$',
    '^PRD/.+\.md$',
    '^examples/[^/]+\.rs$',
    '^frontend/(?:index\.html|package(?:-lock)?\.json|tsconfig\.json)$',
    '^frontend/src/.+\.(?:ts|css)$',
    '^proto/[^/]+\.proto$',
    '^scripts/[^/]+\.ps1$',
    '^src/.+\.rs$',
    '^tools/protoc/readme\.txt$'
)

$deniedPatterns = @(
    '(?i)(^|/)(?:credentials\.json|\.env(?:\..*)?)$',
    '(?i)(^|/)config(?:\.[^/]+)?\.local\.toml$',
    '(?i)(^|/)(?:target|dist|logs|\.idea|\.vscode)/',
    '(?i)\.(?:pem|key|pfx|p12|log|dmp|stackdump|bak|orig|exe|dll|pdb)$',
    '~$'
)

$repoRoot = (& git rev-parse --show-toplevel).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "Unable to locate the repository root."
}

$trackedFiles = @(git -c core.quotepath=false ls-files | Where-Object {
    Test-Path -LiteralPath (Join-Path $repoRoot $_) -PathType Leaf
})
if ($LASTEXITCODE -ne 0) {
    throw "Unable to enumerate tracked files."
}

$violations = @(
    foreach ($file in $trackedFiles) {
        $path = $file.Replace('\', '/')
        $isAllowed = $allowedPatterns.Where({ $path -match $_ }, 'First').Count -gt 0
        $isDenied = $deniedPatterns.Where({ $path -match $_ }, 'First').Count -gt 0

        if (-not $isAllowed -or $isDenied) {
            $path
        }
    }
)

if ($violations.Count -gt 0) {
    Write-Error "Repository allowlist rejected tracked files:`n$($violations -join "`n")"
    exit 1
}

Write-Host "Repository allowlist passed ($($trackedFiles.Count) tracked files)."
