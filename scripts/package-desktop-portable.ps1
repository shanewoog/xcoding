# Build a portable (green) XCoding Desktop folder: no installer required.
# Output: dist/portable/XCoding/
# Usage:
#   .\scripts\package-desktop-portable.ps1
#   .\scripts\package-desktop-portable.ps1 -SkipBuild
#   .\scripts\package-desktop-portable.ps1 -TestVersionRule

param(
  [switch]$SkipBuild,
  [switch]$TestVersionRule
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
$versionFile = Join-Path $Root "VERSION"
$outDir = Join-Path $Root "dist\portable\XCoding"
$frontendIndex = Join-Path $Root "apps\desktop\dist\index.html"
$releaseCandidates = @(
  (Join-Path $Root "apps\desktop\src-tauri\target\release\xcoding-desktop.exe"),
  (Join-Path $Root "apps\desktop\src-tauri\target\release\XCoding.exe")
)

if (-not (Test-Path $versionFile)) {
  throw "Version file missing: $versionFile"
}
$version = (Get-Content -LiteralPath $versionFile -Raw).Trim()
if ($version -notmatch '^\d+\.\d+\.\d+$') {
  throw "Invalid version in ${versionFile}: $version"
}

function Get-NormalizedVersion([string]$Value) {
  if ($Value -notmatch '^(\d+)\.(\d+)\.(\d+)$') {
    throw "Invalid version: $Value"
  }
  $major = [int]$matches[1]
  $minor = [int]$matches[2]
  $patch = [int]$matches[3]
  $carry = [int][math]::Floor($patch / 100)
  if ($carry -eq 0) {
    return $Value
  }
  $minor += $carry
  $patch = $patch % 100
  return "$major.$minor.$patch"
}

function Get-NextVersion([string]$Value) {
  if ($Value -notmatch '^(\d+)\.(\d+)\.(\d+)$') {
    throw "Invalid version: $Value"
  }
  $parts = $Value.Split('.')
  $major = [int]$parts[0]
  $minor = [int]$parts[1]
  $patch = [int]$parts[2]
  if ($patch -ge 99) {
    $minor += 1
    $patch = 0
  } else {
    $patch += 1
  }
  return "$major.$minor.$patch"
}

if ($TestVersionRule) {
  $cases = @(
    @{ Input = "1.0.100"; Normalized = "1.1.0" },
    @{ Input = "1.0.102"; Normalized = "1.1.2" },
    @{ Input = "1.2.100"; Normalized = "1.3.0" }
  )
  foreach ($case in $cases) {
    $actual = Get-NormalizedVersion $case.Input
    if ($actual -ne $case.Normalized) {
      throw "Version normalization failed: $($case.Input) -> $actual, expected $($case.Normalized)"
    }
  }
  $nextCases = @(
    @{ Input = "1.0.98"; Next = "1.0.99" },
    @{ Input = "1.0.99"; Next = "1.1.0" },
    @{ Input = "1.0.100"; Next = "1.1.0" },
    @{ Input = "1.1.0"; Next = "1.1.1" },
    @{ Input = "1.1.99"; Next = "1.2.0" }
  )
  foreach ($case in $nextCases) {
    $actual = Get-NextVersion $case.Input
    if ($actual -ne $case.Next) {
      throw "Next-version rule failed: $($case.Input) -> $actual, expected $($case.Next)"
    }
  }
  Write-Host "Version rule self-test passed."
  return
}

$version = Get-NormalizedVersion $version

function Update-VersionField([string]$Path, [string]$Pattern, [string]$Version) {
  $content = [System.IO.File]::ReadAllText($Path)
  $matches = [regex]::Matches($content, $Pattern)
  if ($matches.Count -ne 1) {
    throw "Expected exactly one version field in $Path, found $($matches.Count)"
  }
  $updated = [regex]::Replace($content, $Pattern, {
      param($match)
      $match.Groups[1].Value + $Version + $match.Groups[2].Value
    }, 1)
  [System.IO.File]::WriteAllText($Path, $updated, [System.Text.UTF8Encoding]::new($false))
}

Update-VersionField (Join-Path $Root "apps\desktop\package.json") '(?m)^(\s*"version"\s*:\s*")[^"]+(".*)$' $version
Update-VersionField (Join-Path $Root "package.json") '(?m)^(\s*"version"\s*:\s*")[^"]+(".*)$' $version
Update-VersionField (Join-Path $Root "apps\desktop\src-tauri\Cargo.toml") '(?ms)\A(\[package\].*?^version\s*=\s*")[^"]+(".*)$' $version
Update-VersionField (Join-Path $Root "apps\desktop\src-tauri\tauri.conf.json") '(?m)^(\s*"version"\s*:\s*")[^"]+(".*)$' $version
Update-VersionField (Join-Path $Root "apps\desktop\src-tauri\Cargo.lock") '(?ms)(name = "xcoding-desktop"\r?\nversion = ")[^"]+(".*?dependencies = \[)' $version

# Stop a running portable/desktop process so the release binary can be overwritten.
# Remember whether XCoding.exe was running so it can be restarted after packaging.
$xcodingProcesses = @(Get-Process -Name "XCoding" -ErrorAction SilentlyContinue)
$restartXcodingAfterPackage = $xcodingProcesses.Count -gt 0
$desktopProcesses = @(Get-Process -Name "xcoding-desktop" -ErrorAction SilentlyContinue)
@($xcodingProcesses + $desktopProcesses) | Stop-Process -Force -ErrorAction SilentlyContinue

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
if ($restartXcodingAfterPackage) {
  Start-Process -FilePath $destExe -WorkingDirectory $outDir
  Write-Host "Restarted XCoding after packaging."
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

Version: $version

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
Write-Host "  Version: $version"
Get-Item $destExe | Format-List Name, Length, FullName

$nextVersion = Get-NextVersion $version
[System.IO.File]::WriteAllText($versionFile, "$nextVersion`n", [System.Text.UTF8Encoding]::new($false))
Write-Host "  Next release version: $nextVersion"

# Publish the release metadata and source changes. Only the explicit allowlist is staged,
# so unrelated untracked files such as local .env files or generated artifacts stay out.
$releaseGitPaths = @(
  "VERSION",
  "package.json",
  "apps/desktop/package.json",
  "pnpm-lock.yaml",
  "apps/desktop/src-tauri/Cargo.toml",
  "apps/desktop/src-tauri/Cargo.lock",
  "apps/desktop/src-tauri/tauri.conf.json",
  "apps/desktop/src-tauri/capabilities/default.json",
  "apps/desktop/src-tauri/src/main.rs",
  "apps/desktop/src-tauri/src/projects.rs",
  "apps/desktop/src-tauri/src/project_cleanup.rs",
  "apps/desktop/src-tauri/src/workspace_tools.rs",
  "apps/desktop/src-tauri/src/relay.rs",
  "apps/desktop/src-tauri/src/request_logs.rs",
  "apps/desktop/src-tauri/src/terminal.rs",
  "crates/xcoding-agent/Cargo.toml",
  "crates/xcoding-agent/examples/desktop_trajectory.rs",
  "Cargo.toml",
  "Cargo.lock",
  "tests/e2e/desktop-config.mjs",
  "tests/e2e/desktop-context-usage.mjs",
  "tests/e2e/running-cancel-agent.mjs",
  "tests/acceptance/run.mjs",
  "crates/xcoding-agent/src/lib.rs",
  "crates/xcoding-server/src/main.rs",
  "crates/xcoding-core/src/lib.rs",
  "crates/xcoding-context/Cargo.toml",
  "crates/xcoding-context/src/compression.rs",
  "crates/xcoding-context/src/lib.rs",
  "crates/xcoding-store/Cargo.toml",
  "crates/xcoding-store/src/lib.rs",
  "crates/xcoding-tools/src/lib.rs",
  "crates/xcoding-policy/src/lib.rs",
  "crates/xcoding-protocol/src/lib.rs",
  "crates/xcoding-providers/Cargo.toml",
  "crates/xcoding-providers/src/lib.rs",
  "crates/xcoding-providers/src/request_logs.rs",
  "crates/xcoding-providers/tests/request_logging.rs",
  "packages/protocol/src/index.ts",
  "apps/desktop/src/i18n.ts",
  "tests/e2e/provider-retry.mjs",
  "tests/e2e/prompt-cache-prefix.mjs",
  "docs/en/getting-started.md",
  "docs/en/session-safety.md",
  "docs/zh/getting-started.md",
  "docs/zh/session-safety.md",
  "tests/e2e/full-auto-mode.mjs",
  "apps/desktop/src/App.tsx",
  "apps/desktop/src/ProjectCleanup.tsx",
  "apps/desktop/src/panels.tsx",
  "apps/desktop/src/workspaceApi.ts",
  "apps/desktop/src/RequestLogs.tsx",
  "apps/desktop/src/RequestLogs.css",
  "tests/e2e/desktop-request-logs.mjs",
  "docs/model-request-logs.md",
  "apps/desktop/src/activity.ts",
  "apps/desktop/src/styles.css",
  "tests/e2e/desktop-history-auto-load.mjs",
  "tests/e2e/desktop-theme.mjs",
  "tests/e2e/desktop-layout.mjs",
  "tests/e2e/desktop-project-collapse.mjs",
  "tests/e2e/desktop-file-viewer.mjs",
  "tests/e2e/desktop-review.mjs",
  "tests/e2e/desktop-activity.mjs",
  "tests/e2e/desktop-cancel.mjs",
  "tests/e2e/desktop-session-management.mjs",
  "tests/e2e/desktop-project-cleanup.mjs",
  "tests/e2e/desktop-message-links.mjs",
  "tests/e2e/desktop-follow-up-queue.mjs",
  "tests/e2e/desktop-composer-isolation.mjs",
  "tests/e2e/model-call-logs.mjs",
  "tests/e2e/read-only-agent.mjs",
  "tests/e2e/git-tools-agent.mjs",
  "tests/e2e/git-write-agent.mjs",
  "tests/e2e/skills-agent.mjs",
  "tests/e2e/anthropic-messages.mjs",
  "tests/e2e/dynamic-surface-parity.mjs",
  "tests/e2e/README.md",
  "tests/e2e/run.mjs",
  "tests/e2e/custom-command-allowlist.mjs",
  "tests/e2e/provider-auth-error.mjs",
  "tests/e2e/vision-delegate.mjs",
  "apps/desktop/src/layout.ts",
  "tests/e2e/write-loop-agent.mjs",
  "tests/e2e/model-plan-agent.mjs",
  "tests/e2e/fixtures/model-plan-agent",
  "tests/acceptance/README.md",
  "docs/en/protocol.md",
  "docs/desktop.md",
  "docs/en/roadmap.md",
  "docs/zh/protocol.md",
  "docs/zh/desktop.md",
  "docs/zh/roadmap.md",
  "scripts/package-desktop-portable.ps1"
)

function Invoke-Git([string[]]$Arguments) {
  & git @Arguments
  if ($LASTEXITCODE -ne 0) {
    throw "git $($Arguments -join ' ') failed with exit $LASTEXITCODE"
  }
}

# The release paths must reach git as part of one argument array. Writing
# `Invoke-Git @(...) + $paths` binds only the literal array and discards the
# rest, which drops the pathspec and lets git fall back to the whole worktree.
Invoke-Git (@("diff", "--check", "--") + $releaseGitPaths)
Invoke-Git (@("diff", "--cached", "--check", "--") + $releaseGitPaths)

$branch = (& git rev-parse --abbrev-ref HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($branch) -or $branch -eq "HEAD") {
  throw "Cannot publish automatically from a detached HEAD"
}
& git remote get-url origin *> $null
if ($LASTEXITCODE -ne 0) {
  throw "Git remote 'origin' is not configured; release was not pushed"
}

Invoke-Git (@("add", "--") + $releaseGitPaths)
Invoke-Git (@("diff", "--cached", "--check", "--") + $releaseGitPaths)

$hasReleaseChanges = $false
& git diff --quiet HEAD -- $releaseGitPaths
if ($LASTEXITCODE -eq 1) {
  $hasReleaseChanges = $true
} elseif ($LASTEXITCODE -ne 0) {
  throw "Could not determine release changes"
}

if ($hasReleaseChanges) {
  Invoke-Git (@("commit", "--only", "-m", "release: publish XCoding v$version", "--") + $releaseGitPaths)
} else {
  Write-Host "No release file changes to commit."
}

Invoke-Git @("push", "origin", $branch)
Write-Host "Published Git commit to origin/$branch"
