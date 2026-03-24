$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$binaryPath = Join-Path $repoRoot "src-tauri\target\debug\mirror-windows.exe"

$staleProcesses = Get-Process -Name "mirror-windows" -ErrorAction SilentlyContinue | Where-Object {
  $_.Path -eq $binaryPath
}

foreach ($process in $staleProcesses) {
  Stop-Process -Id $process.Id -Force
  Wait-Process -Id $process.Id -Timeout 5 -ErrorAction SilentlyContinue
}

& npm.cmd run dev
