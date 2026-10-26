<#
.SYNOPSIS
  Builds the Windows installer (setup.exe) with Inno Setup.

.DESCRIPTION
  Reads the version from the workspace Cargo.toml, builds the release binaries (unless -NoBuild),
  and runs the Inno Setup compiler on installer/j2k-viewer.iss. The result is
  dist/j2k-viewer-<version>-windows-<arch>-setup.exe.

  Needs Inno Setup 6:  winget install --id JRSoftware.InnoSetup -e --scope user

.EXAMPLE
  scripts/build-installer.ps1
  scripts/build-installer.ps1 -NoBuild          # reuse target\release
  scripts/build-installer.ps1 -Arch aarch64 -BinDir target\aarch64-pc-windows-msvc\release
#>
param(
    [ValidateSet('x86_64', 'aarch64')] [string] $Arch = 'x86_64',
    [string] $BinDir = 'target\release',
    [switch] $NoBuild
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $version) { throw 'could not read the version from Cargo.toml' }

$iscc = @(
    (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source,
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 not found. Install it with: winget install --id JRSoftware.InnoSetup -e --scope user' }

if (-not $NoBuild) {
    $cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
    if (-not $cargo) { $cargo = "$env:USERPROFILE\.cargo\bin\cargo.exe" }
    Write-Host "Building release binaries..."
    & $cargo build --release --locked -p viewer -p tools
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
}
foreach ($exe in 'viewer.exe', 'tools.exe') {
    if (-not (Test-Path (Join-Path $BinDir $exe))) { throw "$BinDir\$exe not found" }
}

New-Item -ItemType Directory -Force dist | Out-Null
$binAbs = (Resolve-Path $BinDir).Path
Write-Host "Compiling installer for version $version ($Arch)..."
& $iscc "/DAppVersion=$version" "/DArch=$Arch" "/DBinDir=$binAbs" 'installer\j2k-viewer.iss'
if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed ($LASTEXITCODE)" }

$out = "dist\j2k-viewer-$version-windows-$Arch-setup.exe"
if (-not (Test-Path $out)) { throw "expected output $out was not produced" }
Write-Host ("Built {0} ({1:N1} MB)" -f $out, ((Get-Item $out).Length / 1MB))
