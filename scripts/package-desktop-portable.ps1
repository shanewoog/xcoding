# Build a portable (green) XCoding Desktop folder: no installer required.
# Output: dist/portable/XCoding/
# Usage:
#   .\scripts\package-desktop-portable.ps1
#   .\scripts\package-desktop-portable.ps1 -SkipBuild   # repack existing release binary

param(
  [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $Root

function Ensure-PathPrefix([string]$Prefix) {
  if (Test-Path $Prefix) {
    if (-not (($env:Path -split ";") -contains $Prefix)) {
      $env:Path = "$Prefix;" + $env:Path
    }
  }
}

Ensure-PathPrefix "D:\WORK\Npm"
Ensure-PathPrefix "$env:APPDATA\npm"
Ensure-PathPrefix "C:\Program Files\nodejs"

$pnpm = if (Test-Path "D:\WORK\Npm\pnpm.cmd") { "D:\WORK\Npm\pnpm.cmd" } else { "pnpm" }
$outDir = Join-Path $Root "dist\portable\XCoding"
$releaseCandidates = @(
  (Join-Path $Root "apps\desktop\src-tauri\target\release\xcoding-desktop.exe"),
  (Join-Path $Root "apps\desktop\src-tauri\target\release\XCoding.exe")
)

if (-not $SkipBuild) {
  Write-Host "Building frontend + Tauri release binary (no installer bundle)..."
  & $pnpm --filter @xcoding/desktop exec tauri build --no-bundle
  if ($LASTEXITCODE -ne 0) { throw "tauri build failed with exit $LASTEXITCODE" }
}

$exe = $releaseCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $exe) {
  throw "Release binary not found. Expected one of:`n$($releaseCandidates -join "`n")"
}

if (Test-Path $outDir) {
  Remove-Item -LiteralPath $outDir -Recurse -Force
}
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

$destExe = Join-Path $outDir "XCoding.exe"
Copy-Item -LiteralPath $exe -Destination $destExe -Force

$envExample = @"
# Put this file next to XCoding.exe and rename to .env
# Existing process environment variables still win over this file.

OPENAI_API_KEY=sk-your-key-here
XCODING_OPENAI_BASE_URL=https://ai.v58.dev/v1
"@
Set-Content -Path (Join-Path $outDir ".env.example") -Value $envExample -Encoding utf8

$readme = @"
# XCoding Desktop (Portable / Green)

## Run

1. Copy this folder anywhere (USB / local disk).
2. Copy `.env.example` to `.env` and fill in your API key.
3. Double-click `XCoding.exe`.

No installer is required.

## Requirements

- Windows 10/11 with WebView2 Runtime (usually preinstalled)
- Network access to your cloud model endpoint

## Notes

- Session database is stored under the OS app data directory, not inside this folder.
- Do not commit real API keys.
- Prefer `ask` mode for first runs.
"@
Set-Content -Path (Join-Path $outDir "README.txt") -Value $readme -Encoding utf8

Write-Host ""
Write-Host "Portable package ready:"
Write-Host "  $outDir"
Write-Host "  $destExe"
Get-Item $destExe | Format-List Name, Length, FullName

