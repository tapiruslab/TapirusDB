# ==============================================================================
# TapirusDB Universal Installer for Windows (PowerShell)
# Architected by Ahmad Faiz • Tapirus Tech Lab (TapirusDB.com)
# ==============================================================================
# Usage:
#   irm https://raw.githubusercontent.com/TapirusDB/tapirus/main/install.ps1 | iex
# ==============================================================================

$ErrorActionPreference = "Stop"

$TapirusVersion = "0.1.2"
$Repo = "tapiruslab/TapirusDB"
$InstallDir = if ($env:TAPIRUS_INSTALL_DIR) { $env:TAPIRUS_INSTALL_DIR } else { "$env:USERPROFILE\.tapirus" }
$BinDir = "$InstallDir\bin"
$IncludeDir = "$InstallDir\include"

Write-Host @"
  ___________           .__                     ________  __________
  \__    ___/____  ____ |__|______ __ __  ______\______ \ \______   \
    |    |  \__  \ \____ \|  \_  __ \  |  \/  ___/ |    |  \ |    |  _/
    |    |   / __ \|  |_> >  ||  | \/  |  /\___ \  |    `   \|    |   \
    |____|  (____  /   __/|__||__|  |____//____  >/_______  /|______  /
                 \/|__|                        \/         \/        \/
"@ -ForegroundColor Cyan

Write-Host "`n==> TapirusDB Universal Windows Installer (v$TapirusVersion)" -ForegroundColor Green
Write-Host "The Safe-Rust Embedded Quad-Model AI Database Engine.`n"

# 1. Ensure Install Directories Exist
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
New-Item -ItemType Directory -Force -Path $IncludeDir | Out-Null

# 2. Check if Cargo/Rust is installed
$hasCargo = Get-Command cargo -ErrorAction SilentlyContinue

if ($hasCargo) {
    Write-Host "==> Rust toolchain detected. Compiling TapirusDB native binaries..." -ForegroundColor Cyan
    $tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $tempDir | Out-Null

    try {
        git clone --depth 1 "https://github.com/$Repo.git" "$tempDir\tapirus" | Out-Null
        Set-Location "$tempDir\tapirus"
        cargo build --release --workspace

        Copy-Item "target\release\tapirus.exe" -Destination "$BinDir\tapirus.exe" -Force
        if (Test-Path "target\release\tapirus.dll") {
            Copy-Item "target\release\tapirus.dll" -Destination "$BinDir\tapirus.dll" -Force
        }
        Copy-Item "include\tapirus.h" -Destination "$IncludeDir\tapirus.h" -Force
    }
    finally {
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tempDir
    }
}
else {
    Write-Host "==> Downloading pre-compiled Windows release from GitHub..." -ForegroundColor Cyan
    $zipUrl = "https://github.com/$Repo/releases/download/v$TapirusVersion/tapirus-windows-x86_64.zip"
    $tempZip = Join-Path ([System.IO.Path]::GetTempPath()) "tapirus.zip"

    try {
        Invoke-WebRequest -Uri $zipUrl -OutFile $tempZip -UseBasicParsing
        Expand-Archive -Path $tempZip -DestinationPath $InstallDir -Force
        Remove-Item -Force $tempZip
    }
    catch {
        Write-Warning "Could not download pre-built release. Please install Rust from https://rustup.rs and rerun this installer."
        exit 1
    }
}

# 3. Add to User PATH Environment Variable
$userPath = [Environment]::GetEnvironmentVariable("Path", [EnvironmentVariableTarget]::User)
if ($userPath -notlike "*$BinDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$BinDir", [EnvironmentVariableTarget]::User)
    $env:PATH = "$env:PATH;$BinDir"
    Write-Host "✓ Added $BinDir to User PATH." -ForegroundColor Green
}

Write-Host @"

🎉 TapirusDB successfully installed on Windows!
Binary executable: $BinDir\tapirus.exe
Shared DLL:        $BinDir\tapirus.dll
C Header:          $IncludeDir\tapirus.h

Run in your terminal:
  tapirus --help
"@ -ForegroundColor Green
