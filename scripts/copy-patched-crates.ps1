$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$patchDir = Join-Path $Root "patches"
New-Item -ItemType Directory -Path $patchDir -Force | Out-Null

$registryBase = Join-Path $env:USERPROFILE ".cargo\registry\src\index.crates.io-1949cf8c6b5b557f"
$crates = @("wry-0.55.1", "tauri-runtime-2.11.3", "tauri-runtime-wry-2.11.4", "tauri-2.11.5")

foreach ($name in $crates) {
    $src = Join-Path $registryBase $name
    $dest = Join-Path $patchDir $name
    if (-not (Test-Path $src)) { throw "Source not found: $src" }
    if (Test-Path $dest) { Remove-Item -LiteralPath $dest -Recurse -Force }
    Copy-Item -LiteralPath $src -Destination $dest -Recurse -Force
    Get-ChildItem -LiteralPath $dest -Recurse -Filter "*.bak" -ErrorAction SilentlyContinue | Remove-Item -Force
    $targetDir = Join-Path $dest "target"
    if (Test-Path $targetDir) { Remove-Item -LiteralPath $targetDir -Recurse -Force }
    Write-Host "Copied $name"
}
Write-Host "Done. Patched crates saved to $patchDir"
