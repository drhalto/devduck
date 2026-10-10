# Build Dev Ports and install it for the current user:
#   devports.exe + devports-app.exe -> ~\.local\bin (or -Dest), plus a "Dev Ports" Start menu shortcut.
# Re-run after pulling changes. A running Dev Ports window is closed first so the exe can be replaced.
param([string]$Dest = (Join-Path $env:USERPROFILE '.local\bin'), [switch]$NoBuild)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent

if (-not $NoBuild) {
    Push-Location $repo
    try { cargo build --release; if ($LASTEXITCODE) { throw "cargo build failed" } } finally { Pop-Location }
}

foreach ($p in Get-Process devports-app -ErrorAction SilentlyContinue) {
    [void]$p.CloseMainWindow()
    if (-not $p.WaitForExit(3000)) { Stop-Process -Id $p.Id -Force }
}

New-Item -ItemType Directory -Force $Dest | Out-Null
foreach ($exe in 'devports.exe', 'devports-app.exe') {
    Copy-Item (Join-Path $repo "target\release\$exe") (Join-Path $Dest $exe) -Force
}

$programs = [Environment]::GetFolderPath('Programs')
$shell = New-Object -ComObject WScript.Shell
$lnk = $shell.CreateShortcut((Join-Path $programs 'Dev Ports.lnk'))
$lnk.TargetPath = Join-Path $Dest 'devports-app.exe'
$lnk.WorkingDirectory = $env:USERPROFILE
$lnk.Description = "See what's listening on your local ports, grouped by project"
$lnk.IconLocation = "$(Join-Path $Dest 'devports-app.exe'),0"
$lnk.Save()

if (-not (($env:PATH -split ';') -contains $Dest)) {
    Write-Warning "$Dest isn't on your PATH; add it to use the devports command."
}
"Installed to $Dest. Start menu: Dev Ports. CLI: devports --help"
