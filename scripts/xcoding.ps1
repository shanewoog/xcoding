# XCoding local launcher for Windows PowerShell.
# Usage:
#   .\scripts\xcoding.ps1 ping
#   .\scripts\xcoding.ps1 chat "Explain this repo"
#   .\scripts\xcoding.ps1 desktop
#   .\scripts\xcoding.ps1 acceptance

param(
  [Parameter(Position = 0)]
  [string]$Command = "help",
  [Parameter(ValueFromRemainingArguments = $true)]
  [string[]]$Rest
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

function Import-DotEnv([string]$Path) {
  if (-not (Test-Path $Path)) { return }
  Get-Content -LiteralPath $Path | ForEach-Object {
    $line = $_.Trim()
    if (-not $line -or $line.StartsWith("#") -or -not $line.Contains("=")) { return }
    $parts = $line.Split("=", 2)
    $name = $parts[0].Trim()
    $value = $parts[1].Trim()
    if (($value.StartsWith('"') -and $value.EndsWith('"')) -or ($value.StartsWith("'") -and $value.EndsWith("'"))) {
      $value = $value.Substring(1, $value.Length - 2)
    }
    $existing = [Environment]::GetEnvironmentVariable($name, "Process")
    if ([string]::IsNullOrEmpty($existing)) {
      Set-Item -Path "Env:$name" -Value $value
    }
  }
}

Import-DotEnv (Join-Path $Root ".env")

function Resolve-Pnpm {
  $candidates = @(
    "pnpm",
    "D:\WORK\Npm\pnpm.cmd",
    "$env:APPDATA\npm\pnpm.cmd"
  )
  foreach ($item in $candidates) {
    try {
      if ($item -eq "pnpm") {
        $cmd = Get-Command pnpm -ErrorAction SilentlyContinue
        if ($cmd) { return $cmd.Source }
      } elseif (Test-Path $item) {
        return $item
      }
    } catch {}
  }
  throw "pnpm not found. Install with: npm install -g pnpm@11.9.0"
}

$pnpm = Resolve-Pnpm

switch ($Command.ToLowerInvariant()) {
  "help" {
    @"
XCoding launcher

Commands:
  ping                         Health check against local core
  chat <message>               Chat with the agent in current repo
  config show|set ...          Workspace config
  desktop                      Start Tauri desktop shell
  build                        Build server + JS packages
  acceptance                   Run V1 acceptance harness
  cli <args...>                Pass-through to CLI

Examples:
  .\scripts\xcoding.ps1 chat "说明这个仓库"
  .\scripts\xcoding.ps1 desktop
  .\scripts\xcoding.ps1 acceptance
"@
  }
  "ping" {
    & $pnpm cli -- ping --workspace $Root
  }
  "chat" {
    if (-not $Rest -or -not $Rest[0]) { throw "chat requires a message" }
    & $pnpm cli -- chat $Rest[0] --workspace $Root @($Rest | Select-Object -Skip 1)
  }
  "config" {
    & $pnpm cli -- config @Rest --workspace $Root
  }
  "desktop" {
    & $pnpm desktop
  }
  "build" {
    cargo build -p xcoding-server
    & $pnpm build
  }
  "acceptance" {
    & $pnpm run test:acceptance
  }
  "cli" {
    & $pnpm cli -- @Rest
  }
  default {
    & $pnpm cli -- $Command @Rest
  }
}
