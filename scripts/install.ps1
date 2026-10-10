# Build DevDuck and install it for the current user:
#   devduck.exe + devduck-app.exe -> ~\.local\bin (or -Dest), plus a "DevDuck" Start menu shortcut.
# Re-run after pulling changes. A running DevDuck window is closed first so the exe can be replaced.
# Also removes the pre-rename "Dev Ports" exes and shortcut if they're still around.
param([string]$Dest = (Join-Path $env:USERPROFILE '.local\bin'), [switch]$NoBuild)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent

if (-not $NoBuild) {
    Push-Location $repo
    try { cargo build --release; if ($LASTEXITCODE) { throw "cargo build failed" } } finally { Pop-Location }
}

foreach ($p in Get-Process devduck-app, devports-app -ErrorAction SilentlyContinue) {
    [void]$p.CloseMainWindow()
    if (-not $p.WaitForExit(3000)) { Stop-Process -Id $p.Id -Force }
}

New-Item -ItemType Directory -Force $Dest | Out-Null
foreach ($exe in 'devduck.exe', 'devduck-app.exe') {
    Copy-Item (Join-Path $repo "target\release\$exe") (Join-Path $Dest $exe) -Force
}

$programs = [Environment]::GetFolderPath('Programs')
foreach ($old in (Join-Path $Dest 'devports.exe'), (Join-Path $Dest 'devports-app.exe'), (Join-Path $programs 'Dev Ports.lnk')) {
    if (Test-Path $old) { Remove-Item $old -Force; "Removed old $old" }
}

$shell = New-Object -ComObject WScript.Shell
$lnk = $shell.CreateShortcut((Join-Path $programs 'DevDuck.lnk'))
$lnk.TargetPath = Join-Path $Dest 'devduck-app.exe'
$lnk.WorkingDirectory = $env:USERPROFILE
$lnk.Description = 'Every dev server on your PC, grouped by project'
$lnk.IconLocation = "$(Join-Path $Dest 'devduck-app.exe'),0"
$lnk.Save()

if (-not (($env:PATH -split ';') -contains $Dest)) {
    Write-Warning "$Dest isn't on your PATH; add it to use the devduck command."
}
"Installed to $Dest. Start menu: DevDuck. CLI: devduck --help"
