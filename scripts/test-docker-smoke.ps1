param(
    [string]$Image = 'tapirusdb:local-smoke',
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

if (-not $SkipBuild) {
    & docker build --tag $Image --file (Join-Path $repoRoot 'Dockerfile') $repoRoot
    if ($LASTEXITCODE -ne 0) { throw 'Docker build failed' }
}

# The image declares /data as a volume; --rm also removes its anonymous volume.
$commands = @'
set -eu
test "$(id -u)" != 0
tapirus --version
tapirus /data/smoke.tapir -c 'CREATE TABLE audit (n INTEGER); INSERT INTO audit VALUES (42)'
result="$(tapirus /data/smoke.tapir --json 'SELECT n FROM audit')"
test "$result" = '[{"n":{"Integer":42}}]'
if tapirus /data/smoke.tapir -c 'INVALID SQL' >/dev/null 2>&1; then
    echo 'Invalid SQL unexpectedly returned success' >&2
    exit 1
fi
printf 'Docker smoke test passed: non-root, offline, SQL persistence/reopen.\n'
'@

$commands | & docker run --rm -i --network none --read-only --entrypoint sh $Image -s
if ($LASTEXITCODE -ne 0) { throw 'Docker smoke test failed' }
