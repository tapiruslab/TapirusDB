# TapirusDB Windows installer. Prebuilt releases require a matching SHA-256 file.
[CmdletBinding()]
param(
    [string]$Version = '1.0.2',
    [string]$InstallDir = $env:TAPIRUS_INSTALL_DIR,
    [switch]$FromSource,
    [string]$ArchivePath,
    [string]$ChecksumPath,
    [switch]$NoPath
)
$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?$') { throw 'Invalid release version' }
if (-not $InstallDir) { $InstallDir = Join-Path $env:USERPROFILE '.tapirus' }
$InstallDir = [IO.Path]::GetFullPath($InstallDir)
$binDir = Join-Path $InstallDir 'bin'
$includeDir = Join-Path $InstallDir 'include'
$temporaryBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\', '/')
$temporaryDir = Join-Path $temporaryBase ('tapirus-install-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temporaryDir | Out-Null
try {
    if ($FromSource) {
        if ($ArchivePath) { throw 'Choose either -FromSource or -ArchivePath' }
        $sourceDir = Join-Path $temporaryDir 'source'
        & git clone --branch "v$Version" --depth 1 'https://github.com/tapiruslab/TapirusDB.git' $sourceDir
        if ($LASTEXITCODE -ne 0) { throw 'Could not clone the requested release tag' }
        Push-Location $sourceDir
        try {
            & cargo build --locked --release -p tapirus -p tapirus-ffi
            if ($LASTEXITCODE -ne 0) { throw 'Source build failed' }
        } finally { Pop-Location }
        $payloadDir = Join-Path $sourceDir 'target/release'
        $headerPath = Join-Path $sourceDir 'include/tapirus.h'
    } else {
        if (-not $ArchivePath) {
            if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() -ne 'X64') {
                throw 'This prebuilt installer supports Windows x64. Use -FromSource on other architectures.'
            }
            $name = 'tapirus-windows-x86_64.zip'
            $ArchivePath = Join-Path $temporaryDir $name
            $ChecksumPath = "$ArchivePath.sha256"
            $baseUrl = "https://github.com/tapiruslab/TapirusDB/releases/download/v$Version"
            Invoke-WebRequest -Uri "$baseUrl/$name" -OutFile $ArchivePath -UseBasicParsing
            Invoke-WebRequest -Uri "$baseUrl/$name.sha256" -OutFile $ChecksumPath -UseBasicParsing
        }
        if (-not $ChecksumPath) { throw 'A local archive also requires -ChecksumPath' }
        $ArchivePath = (Resolve-Path -LiteralPath $ArchivePath).Path
        $archiveName = [IO.Path]::GetFileName($ArchivePath)
        $manifest = [IO.File]::ReadAllText((Resolve-Path -LiteralPath $ChecksumPath).Path)
        $pattern = '(?mi)^([a-f0-9]{64})\s+\*?' + [regex]::Escape($archiveName) + '\s*$'
        $match = [regex]::Match($manifest, $pattern)
        if (-not $match.Success) { throw "Checksum manifest has no SHA-256 entry for $archiveName" }
        $actual = (Get-FileHash -LiteralPath $ArchivePath -Algorithm SHA256).Hash
        if ($actual -ne $match.Groups[1].Value) { throw 'Archive checksum mismatch. Installation stopped.' }
        $payloadDir = Join-Path $temporaryDir 'payload'
        New-Item -ItemType Directory -Path $payloadDir | Out-Null
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $archive = [IO.Compression.ZipFile]::OpenRead($ArchivePath)
        try {
            $allowedPrefix = [IO.Path]::GetFullPath($payloadDir).TrimEnd('\') + [IO.Path]::DirectorySeparatorChar
            foreach ($entry in $archive.Entries) {
                $entryPath = [IO.Path]::GetFullPath((Join-Path $payloadDir $entry.FullName))
                if (-not $entryPath.StartsWith($allowedPrefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Archive contains a path outside its extraction directory' }
            }
        } finally { $archive.Dispose() }
        Expand-Archive -LiteralPath $ArchivePath -DestinationPath $payloadDir -Force
        $headerPath = Join-Path $payloadDir 'include/tapirus.h'
    }
    $executable = Join-Path $payloadDir 'tapirus.exe'
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) { throw 'Release archive does not contain tapirus.exe at its root' }
    $versionOutput = & $executable --version
    if ($LASTEXITCODE -ne 0 -or ($versionOutput -join ' ') -notmatch [regex]::Escape($Version)) { throw 'The executable did not report the requested engine version' }
    New-Item -ItemType Directory -Force -Path $binDir | Out-Null
    Copy-Item -LiteralPath $executable -Destination (Join-Path $binDir 'tapirus.exe') -Force
    $library = Join-Path $payloadDir 'tapirus.dll'
    if (Test-Path -LiteralPath $library -PathType Leaf) { Copy-Item -LiteralPath $library -Destination (Join-Path $binDir 'tapirus.dll') -Force }
    if (Test-Path -LiteralPath $headerPath -PathType Leaf) {
        New-Item -ItemType Directory -Force -Path $includeDir | Out-Null
        Copy-Item -LiteralPath $headerPath -Destination (Join-Path $includeDir 'tapirus.h') -Force
    }
    if (-not $NoPath) {
        $currentPath = [Environment]::GetEnvironmentVariable('Path', [EnvironmentVariableTarget]::User)
        $entries = @($currentPath -split ';' | Where-Object { $_ })
        if (-not ($entries | Where-Object { $_.TrimEnd('\') -eq $binDir.TrimEnd('\') })) {
            [Environment]::SetEnvironmentVariable('Path', (($entries + $binDir) -join ';'), [EnvironmentVariableTarget]::User)
        }
        if (-not (($env:PATH -split ';') -contains $binDir)) { $env:PATH += ";$binDir" }
    }
    Write-Host "Installed TapirusDB v$Version at $binDir. Run: tapirus --help"
} finally {
    # Delete only the freshly allocated installer directory under the temp root.
    $resolvedTemporary = [IO.Path]::GetFullPath($temporaryDir)
    $temporaryPrefix = $temporaryBase + [IO.Path]::DirectorySeparatorChar
    if (-not $resolvedTemporary.StartsWith($temporaryPrefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Refusing cleanup outside the installer temp root' }
    if (Test-Path -LiteralPath $resolvedTemporary) { Remove-Item -LiteralPath $resolvedTemporary -Recurse -Force }
}
