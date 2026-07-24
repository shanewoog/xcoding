# Build a portable (green) XCoding Desktop folder: no installer required.
# Output: dist/portable/XCoding/
# Usage:
#   .\scripts\package-desktop-portable.ps1
#   .\scripts\package-desktop-portable.ps1 -SkipBuild

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
$frontendIndex = Join-Path $Root "apps\desktop\dist\index.html"
$releaseCandidates = @(
  (Join-Path $Root "apps\desktop\src-tauri\target\release\xcoding-desktop.exe"),
  (Join-Path $Root "apps\desktop\src-tauri\target\release\XCoding.exe")
)

# Stop a running portable/desktop process so the release binary can be overwritten.
Get-Process XCoding, xcoding-desktop -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

if (-not $SkipBuild) {
  Write-Host "Building frontend + Tauri production binary (custom-protocol, no installer)..."
  # IMPORTANT: use tauri CLI so production enables feature `custom-protocol`.
  # Plain `cargo build --release` without custom-protocol loads http://localhost:1420.
  & $pnpm --filter @xcoding/desktop exec tauri build --no-bundle
  if ($LASTEXITCODE -ne 0) { throw "tauri build failed with exit $LASTEXITCODE" }
}

if (-not (Test-Path $frontendIndex)) {
  throw "Frontend dist missing: $frontendIndex"
}

$indexHtml = Get-Content -LiteralPath $frontendIndex -Raw
if ($indexHtml -match 'src="/assets/' -or $indexHtml -match 'href="/assets/') {
  throw "Frontend assets still use absolute /assets paths. Set Vite base to './' and rebuild."
}
if ($indexHtml -notmatch 'src="\./assets/' -and $indexHtml -notmatch "src='\./assets/") {
  throw "Frontend assets are not relative (expected ./assets/...). Portable UI would be blank."
}

$exe = $releaseCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $exe) {
  throw "Release binary not found. Expected one of:`n$($releaseCandidates -join "`n")"
}

if (Test-Path $outDir) {
  try {
    Remove-Item -LiteralPath $outDir -Recurse -Force -ErrorAction Stop
  } catch {
    Write-Host "Warning: could not recreate $outDir (in use). Updating binaries in place."
  }
}
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

$destExe = Join-Path $outDir "XCoding.exe"
try {
  Copy-Item -LiteralPath $exe -Destination $destExe -Force -ErrorAction Stop
} catch {
  $fallback = Join-Path $outDir "XCoding.new.exe"
  Copy-Item -LiteralPath $exe -Destination $fallback -Force
  Write-Host "Warning: XCoding.exe locked; wrote $fallback instead."
  $destExe = $fallback
}
# Keep original name for debugging if needed.
Copy-Item -LiteralPath $exe -Destination (Join-Path $outDir "xcoding-desktop.exe") -Force -ErrorAction SilentlyContinue

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
2. Double-click XCoding.exe.
3. Open Settings and set Base URL + API key (or copy .env.example to .env).

No installer is required. Do NOT need a local Vite/dev server.

## Requirements

- Windows 10/11 with WebView2 Runtime (usually preinstalled)
- Network access to your cloud model endpoint

## If the window is blank / no UI

1. Confirm you are using this package (built by pnpm desktop:portable), not a raw cargo binary.
2. Install/repair WebView2 Runtime.
3. Close all XCoding processes, then delete:
   %LOCALAPPDATA%\com.shanewoog.xcoding\EBWebView
4. Reopen XCoding.exe.

## Notes

- User config and session DB are under %USERPROFILE%\.xcoding\
- Packaging recreates this folder; local .env next to the exe may be wiped on rebuild.
- Do not commit real API keys.
- Prefer ask mode for first runs.
"@
Set-Content -Path (Join-Path $outDir "README.txt") -Value $readme -Encoding utf8

Write-Host ""
Write-Host "Portable package ready:"
Write-Host "  $outDir"
Write-Host "  $destExe"
Get-Item $destExe | Format-List Name, Length, FullName
