param(
    [ValidateSet("x86_64", "arm64", "all")]
    [string]$Architecture = "all"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = $PSScriptRoot
$RepoRoot = Split-Path -Parent (Split-Path -Parent $ScriptDir)
$CargoTomlPath = Join-Path $RepoRoot "Cargo.toml"
$IconPath = Join-Path $RepoRoot "assets\broma.ico"
$NsiPath = Join-Path $ScriptDir "broma.nsi"
$DistDir = Join-Path $RepoRoot "dist"

function Get-PackageVersion {
    param([Parameter(Mandatory = $true)][string]$Path)

    $inPackageSection = $false
    foreach ($line in Get-Content -Path $Path -Encoding UTF8) {
        $trimmed = $line.Trim()
        if ($trimmed -match '^\[(.+)\]$') {
            $inPackageSection = ($matches[1] -eq 'package')
            continue
        }
        if ($inPackageSection -and $trimmed -match '^version\s*=\s*"([^"]+)"') {
            return $matches[1]
        }
    }
    throw "Could not find [package] version in $Path"
}

function Find-MakeNsis {
    $cmd = Get-Command makensis -ErrorAction SilentlyContinue
    if ($null -ne $cmd) {
        return $cmd.Source
    }

    $candidates = @()
    if (-not [string]::IsNullOrWhiteSpace(${env:ProgramFiles(x86)})) {
        $candidates += Join-Path ${env:ProgramFiles(x86)} "NSIS\makensis.exe"
    }
    if (-not [string]::IsNullOrWhiteSpace($env:ProgramFiles)) {
        $candidates += Join-Path $env:ProgramFiles "NSIS\makensis.exe"
    }

    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }

    return $null
}

$Version = Get-PackageVersion -Path $CargoTomlPath
$MakeNsis = Find-MakeNsis
if ($null -eq $MakeNsis) {
    Write-Error "makensis.exe not found. Install NSIS first (e.g. 'winget install NSIS.NSIS') and retry."
    exit 1
}
if (-not (Test-Path -LiteralPath $IconPath -PathType Leaf)) {
    throw "Expected application icon not found at $IconPath"
}

$Targets = @{
    x86_64 = @{ Triple = "x86_64-pc-windows-msvc"; InstallerArch = "x64" }
    arm64  = @{ Triple = "aarch64-pc-windows-msvc"; InstallerArch = "arm64" }
}
$Architectures = if ($Architecture -eq "all") { @("x86_64", "arm64") } else { @($Architecture) }
New-Item -ItemType Directory -Force -Path $DistDir | Out-Null

foreach ($Arch in $Architectures) {
    $Target = $Targets[$Arch].Triple
    $InstallerArch = $Targets[$Arch].InstallerArch
    $InstalledTargets = & rustup target list --installed
    if ($LASTEXITCODE -ne 0) {
        throw "Could not query installed Rust targets."
    }
    if ($InstalledTargets -notcontains $Target) {
        Write-Host "Installing Rust target $Target..." -ForegroundColor Cyan
        & rustup target add $Target
        if ($LASTEXITCODE -ne 0) {
            throw "rustup target add $Target failed with exit code $LASTEXITCODE"
        }
    }

    Write-Host "Building Broma v$Version for $Arch ($Target)..." -ForegroundColor Cyan
    Push-Location $RepoRoot
    try {
        & cargo build --locked --release --target $Target
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build for $Target failed with exit code $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }

    $BinPath = Join-Path $RepoRoot "target\$Target\release\broma.exe"
    if (-not (Test-Path -LiteralPath $BinPath -PathType Leaf)) {
        throw "Expected release binary not found at $BinPath"
    }
    $OutFile = Join-Path $DistDir "Broma-Setup-$Version-$InstallerArch.exe"

    Write-Host "Compiling $InstallerArch NSIS installer with $MakeNsis..." -ForegroundColor Cyan
    & $MakeNsis `
        "-DVERSION=$Version" `
        "-DARCH_SUFFIX=$InstallerArch" `
        "-DBIN_PATH=$BinPath" `
        "-DAPP_ICON_PATH=$IconPath" `
        "-DOUT_FILE=$OutFile" `
        $NsiPath

    if ($LASTEXITCODE -ne 0) {
        throw "makensis for $Arch failed with exit code $LASTEXITCODE"
    }
    if (-not (Test-Path -LiteralPath $OutFile -PathType Leaf)) {
        throw "Expected installer output not found at $OutFile"
    }

    $BinSizeMB = [Math]::Round((Get-Item -LiteralPath $BinPath).Length / 1MB, 2)
    $InstallerSizeMB = [Math]::Round((Get-Item -LiteralPath $OutFile).Length / 1MB, 2)
    Write-Host "Built ${Arch}: $OutFile ($InstallerSizeMB MB; app binary $BinSizeMB MB)" -ForegroundColor Green
}
