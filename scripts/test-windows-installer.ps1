$ErrorActionPreference = 'Stop'
$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$testRoot = Join-Path $workspace ('target/installer-check-' + [Guid]::NewGuid().ToString('N'))
$payload = Join-Path $testRoot 'payload'
New-Item -ItemType Directory -Force -Path $payload | Out-Null
try {
    Copy-Item -LiteralPath (Join-Path $workspace 'target/debug/tapirus.exe') -Destination (Join-Path $payload 'tapirus.exe')
    $archive = Join-Path $testRoot 'tapirus-windows-x86_64.zip'
    Compress-Archive -Path (Join-Path $payload '*') -DestinationPath $archive
    $checksum = "$archive.sha256"
    $digest = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText($checksum, "$digest  tapirus-windows-x86_64.zip`n")
    $destination = Join-Path $testRoot 'installed'
    & (Join-Path $workspace 'install.ps1') -InstallDir $destination -ArchivePath $archive -ChecksumPath $checksum -NoPath
    if (-not (Test-Path -LiteralPath (Join-Path $destination 'bin/tapirus.exe'))) { throw 'The installer did not place the executable in bin' }
    [IO.File]::WriteAllText($checksum, (('0' * 64) + "  tapirus-windows-x86_64.zip`n"))
    $rejected = $false
    try { & (Join-Path $workspace 'install.ps1') -InstallDir (Join-Path $testRoot 'tampered') -ArchivePath $archive -ChecksumPath $checksum -NoPath }
    catch { if ($_.Exception.Message -match 'checksum mismatch') { $rejected = $true } else { throw } }
    if (-not $rejected) { throw 'The installer accepted an invalid checksum' }
    if (Test-Path -LiteralPath (Join-Path $testRoot 'tampered/bin/tapirus.exe')) { throw 'Tampered executable was installed' }
    Write-Host 'Windows installer: isolated local archive install and checksum rejection passed.'
} finally {
    $targetRoot = [IO.Path]::GetFullPath((Join-Path $workspace 'target')).TrimEnd('\') + [IO.Path]::DirectorySeparatorChar
    $resolvedTestRoot = [IO.Path]::GetFullPath($testRoot)
    if (-not $resolvedTestRoot.StartsWith($targetRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Refusing test cleanup outside target' }
    Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force
}
