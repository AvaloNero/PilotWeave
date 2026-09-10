[CmdletBinding()]
param([ValidatePattern('^[a-zA-Z0-9-]{1,80}$')][string]$RunId)
$ErrorActionPreference = 'Stop'
# Reuse the guarded default build, artifact hash and process lifetime.
$arguments = @{Mode='PrepareLive'; LiveTarget='HostObserve'}
if ($RunId) { $arguments.RunId = $RunId }
& (Join-Path $PSScriptRoot 'verify-local.ps1') @arguments
exit $LASTEXITCODE
