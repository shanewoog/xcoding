$base = Join-Path $env:USERPROFILE ".cargo\registry\src\index.crates.io-1949cf8c6b5b557f"
$targets = @("wry-0.55.1", "tauri-runtime-2.11.3", "tauri-runtime-wry-2.11.4", "tauri-2.11.5")
$removed = 0
foreach ($t in $targets) {
    $dir = Join-Path $base $t
    if (Test-Path $dir) {
        Get-ChildItem -LiteralPath $dir -Recurse -Filter "*.bak" -ErrorAction SilentlyContinue | ForEach-Object {
            Remove-Item -LiteralPath $_.FullName -Force
            $removed++
        }
    }
}
Write-Host "Removed $removed .bak files from cargo registry"
