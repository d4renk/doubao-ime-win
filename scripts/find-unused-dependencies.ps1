param(
    [ValidateSet("All", "Packages", "Files")]
    [string]$Scope = "All",

    [ValidateSet("Text", "Json")]
    [string]$Format = "Text",

    [switch]$Strict
)

$ErrorActionPreference = "Stop"
$RepoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$Findings = [System.Collections.Generic.List[object]]::new()

function Get-FullPath([string]$Path, [string]$Base = $RepoRoot) {
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path $Base $Path))
}

function Get-RelativePath([string]$Path) {
    $rootUri = [Uri]((Get-FullPath $RepoRoot).TrimEnd('\') + '\')
    $pathUri = [Uri](Get-FullPath $Path)
    return [Uri]::UnescapeDataString($rootUri.MakeRelativeUri($pathUri).ToString()).Replace('/', '\')
}

function Add-Finding(
    [string]$Kind,
    [string]$Item,
    [ValidateSet("High", "Medium", "Info")][string]$Confidence,
    [string]$Reason
) {
    $Findings.Add([pscustomobject]@{
        Kind       = $Kind
        Item       = $Item
        Confidence = $Confidence
        Reason     = $Reason
    })
}

function Get-RustReachableFiles([object[]]$Roots) {
    $seen = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $queue = [System.Collections.Generic.Queue[object]]::new()
    foreach ($root in $Roots) {
        $queue.Enqueue([pscustomobject]@{ Path = (Get-FullPath $root); IsRoot = $true })
    }

    while ($queue.Count -gt 0) {
        $entry = $queue.Dequeue()
        $path = Get-FullPath $entry.Path
        if (-not (Test-Path -LiteralPath $path -PathType Leaf) -or -not $seen.Add($path)) {
            continue
        }

        $content = Get-Content -LiteralPath $path -Raw
        $directory = Split-Path -Parent $path
        $stem = [System.IO.Path]::GetFileNameWithoutExtension($path)
        if ($entry.IsRoot -or $stem -eq "mod") {
            $moduleBase = $directory
        } else {
            $moduleBase = Join-Path $directory $stem
        }

        $pattern = '(?m)^\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:unsafe\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;'
        foreach ($match in [regex]::Matches($content, $pattern)) {
            $name = $match.Groups[1].Value
            $flat = Join-Path $moduleBase "$name.rs"
            $nested = Join-Path (Join-Path $moduleBase $name) "mod.rs"
            if (Test-Path -LiteralPath $flat -PathType Leaf) {
                $queue.Enqueue([pscustomobject]@{ Path = $flat; IsRoot = $false })
            } elseif (Test-Path -LiteralPath $nested -PathType Leaf) {
                $queue.Enqueue([pscustomobject]@{ Path = $nested; IsRoot = $false })
            }
        }
    }

    return $seen
}

function Join-FileContents([string[]]$Paths) {
    return (($Paths | ForEach-Object {
        if (Test-Path -LiteralPath $_ -PathType Leaf) { Get-Content -LiteralPath $_ -Raw }
    }) -join "`n")
}

function Test-Token([string]$Text, [string]$Token) {
    return [regex]::IsMatch($Text, "(?<![A-Za-z0-9_])$([regex]::Escape($Token))(?![A-Za-z0-9_])")
}

Push-Location $RepoRoot
try {
    $metadataText = (& cargo metadata --format-version 1 --no-deps 2>&1) -join "`n"
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed:`n$metadataText"
    }
    $metadata = $metadataText | ConvertFrom-Json
    $package = $metadata.packages | Where-Object {
        (Get-FullPath $_.manifest_path) -eq (Join-Path $RepoRoot "Cargo.toml")
    } | Select-Object -First 1
    if (-not $package) { throw "Cargo package for $RepoRoot was not found." }

    $normalRoots = @($package.targets | Where-Object { $_.kind -notcontains "custom-build" } | ForEach-Object { $_.src_path })
    $buildRoots = @($package.targets | Where-Object { $_.kind -contains "custom-build" } | ForEach-Object { $_.src_path })
    $normalReachable = Get-RustReachableFiles $normalRoots
    $buildReachable = Get-RustReachableFiles $buildRoots
    $allReachable = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($path in $normalReachable) { [void]$allReachable.Add($path) }
    foreach ($path in $buildReachable) { [void]$allReachable.Add($path) }

    if ($Scope -in @("All", "Packages")) {
        $normalText = Join-FileContents @($normalReachable)
        $buildText = Join-FileContents @($buildReachable)
        foreach ($dependency in $package.dependencies) {
            $crateName = if ($dependency.rename) { $dependency.rename } else { $dependency.name.Replace('-', '_') }
            $searchText = if ($dependency.kind -eq "build") { $buildText } else { $normalText }
            if (-not (Test-Token $searchText $crateName)) {
                $location = if ($dependency.kind -eq "build") { "build.rs" } else { "reachable Rust targets" }
                Add-Finding "Rust package" $dependency.name "High" "No '$crateName' reference exists in $location. References in unreachable modules do not count."
            }
        }

        $packageJsonPath = Join-Path $RepoRoot "frontend\package.json"
        if (Test-Path -LiteralPath $packageJsonPath) {
            $packageJson = Get-Content -LiteralPath $packageJsonPath -Raw | ConvertFrom-Json
            $frontendFiles = @(Get-ChildItem (Join-Path $RepoRoot "frontend") -File -Recurse | Where-Object {
                $_.FullName -notmatch '[\\/](node_modules|dist)[\\/]' -and
                $_.Name -notin @("package.json", "package-lock.json")
            } | ForEach-Object { $_.FullName })
            $frontendText = (Join-FileContents $frontendFiles) + "`n" + ($packageJson.scripts | ConvertTo-Json -Compress)
            $npmDependencies = @()
            foreach ($section in @("dependencies", "devDependencies", "optionalDependencies")) {
                if ($packageJson.$section) {
                    $npmDependencies += $packageJson.$section.PSObject.Properties.Name
                }
            }
            foreach ($name in $npmDependencies | Sort-Object -Unique) {
                $used = Test-Token $frontendText $name
                if ($name -eq "typescript") { $used = $used -or (Test-Token $frontendText "tsc") }
                if ($name -eq "vite") { $used = $used -or (Test-Token $frontendText "vite") }
                if (-not $used) {
                    $confidence = if ($name.StartsWith("@types/")) { "Medium" } else { "High" }
                    $reason = if ($name.StartsWith("@types/")) {
                        "No explicit source, tsconfig, or script reference; @types packages may still contribute ambient types, so verify by removing it and rebuilding."
                    } else {
                        "No import, configuration entry, or package-script command references this direct package."
                    }
                    Add-Finding "npm package" $name $confidence $reason
                }
            }
        }
    }

    if ($Scope -in @("All", "Files")) {
        $rustFiles = @(Get-ChildItem (Join-Path $RepoRoot "src") -Filter "*.rs" -File -Recurse)
        $unreachableRust = @($rustFiles | Where-Object { -not $allReachable.Contains($_.FullName) })
        foreach ($file in $unreachableRust) {
            Add-Finding "Rust file" (Get-RelativePath $file.FullName) "High" "Not reachable through mod declarations from any Cargo target. Cargo does not compile this file."
        }

        $activeIncludes = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
        $inactiveIncludes = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
        foreach ($file in $rustFiles) {
            $content = Get-Content -LiteralPath $file.FullName -Raw
            foreach ($match in [regex]::Matches($content, 'include_(?:bytes|str)!\s*\(\s*"([^"]+)"\s*\)')) {
                $included = Get-FullPath $match.Groups[1].Value $file.DirectoryName
                if ($allReachable.Contains($file.FullName)) { [void]$activeIncludes.Add($included) }
                else { [void]$inactiveIncludes.Add($included) }
            }
        }
        foreach ($included in $inactiveIncludes) {
            if (-not $activeIncludes.Contains($included) -and (Test-Path -LiteralPath $included -PathType Leaf)) {
                Add-Finding "Asset file" (Get-RelativePath $included) "High" "Referenced only by an unreachable Rust module, so it is absent from the current product build."
            }
        }

        $buildText = if (Test-Path (Join-Path $RepoRoot "build.rs")) { Get-Content (Join-Path $RepoRoot "build.rs") -Raw } else { "" }
        $protoRoots = @([regex]::Matches($buildText, '"([^"]+\.proto)"') | ForEach-Object {
            Get-FullPath $_.Groups[1].Value
        } | Sort-Object -Unique)
        $reachableProto = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
        $protoQueue = [System.Collections.Generic.Queue[string]]::new()
        foreach ($root in $protoRoots) { $protoQueue.Enqueue($root) }
        $protoSearchRoots = @((Join-Path $RepoRoot "proto"), (Join-Path $RepoRoot "tools\protoc\include"))
        while ($protoQueue.Count -gt 0) {
            $proto = Get-FullPath $protoQueue.Dequeue()
            if (-not (Test-Path -LiteralPath $proto -PathType Leaf) -or -not $reachableProto.Add($proto)) { continue }
            $content = Get-Content -LiteralPath $proto -Raw
            foreach ($match in [regex]::Matches($content, '(?m)^\s*import\s+(?:public\s+|weak\s+)?"([^"]+)"\s*;')) {
                foreach ($searchRoot in $protoSearchRoots) {
                    $candidate = Get-FullPath $match.Groups[1].Value $searchRoot
                    if (Test-Path -LiteralPath $candidate -PathType Leaf) { $protoQueue.Enqueue($candidate); break }
                }
            }
        }
        $bundledProtoRoot = Join-Path $RepoRoot "tools\protoc\include"
        if (Test-Path -LiteralPath $bundledProtoRoot) {
            foreach ($proto in Get-ChildItem $bundledProtoRoot -Filter "*.proto" -File -Recurse) {
                if (-not $reachableProto.Contains($proto.FullName)) {
                    Add-Finding "Protobuf file" (Get-RelativePath $proto.FullName) "High" "Not imported by any .proto compiled from build.rs; CI obtains protoc includes from the checked-in ZIP instead."
                }
            }
        }
    }

    $ordered = @($Findings | Sort-Object Kind, Item)
    if ($Format -eq "Json") {
        $ordered | ConvertTo-Json -Depth 4
    } elseif ($ordered.Count -eq 0) {
        Write-Host "No unused dependency candidates found." -ForegroundColor Green
    } else {
        $ordered | Format-Table Kind, Confidence, Item, Reason -Wrap -AutoSize
        Write-Host ""
        Write-Host "High = absent from the current build graph; Medium = remove-and-build verification required." -ForegroundColor DarkGray
    }

    if ($Strict -and @($ordered | Where-Object Confidence -eq "High").Count -gt 0) { exit 1 }
} finally {
    Pop-Location
}
