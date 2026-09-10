[CmdletBinding()]
param(
    [ValidateSet('Regression', 'NativeIsolated', 'PrepareLive', 'LiveAcceptance')][string]$Mode = 'Regression',
    [ValidatePattern('^[a-zA-Z0-9-]{1,80}$')][string]$RunId,
    [ValidateSet('HostObserve', 'DedicatedUser')][string]$LiveTarget = 'HostObserve',
    [switch]$Resume,
    [ValidateSet('InstallPackage', 'InstallFirstComponent', 'InstallRemainingComponents', 'SignIn', 'AccountConfirmation', 'ProviderSetup', 'GitHubAuthorization', 'UsageSources')][string]$CompletedAction,
    [string]$ToolRoot = "$env:LOCALAPPDATA\PilotWeave-validation\tools"
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
function Assert-PlainPath([string]$Path) {
    $current = [IO.Path]::GetFullPath($Path)
    while ($current) {
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Validation paths must not traverse junctions or symbolic links' }
        }
        $current = [IO.Path]::GetDirectoryName($current)
    }
}
if (!$RunId) { $RunId = (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + ([guid]::NewGuid().ToString('N').Substring(0,8)) }
$runRoot = Join-Path "$env:LOCALAPPDATA\PilotWeave-validation\runs" $RunId
Assert-PlainPath $runRoot
Assert-PlainPath $ToolRoot
New-Item -ItemType Directory -Path $runRoot -Force | Out-Null
# Evidence can include synthetic target data; isolate it even from other local users.
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
& icacls.exe $runRoot /inheritance:r /grant:r "*${sid}:(OI)(CI)F" '*S-1-5-18:(OI)(CI)F' | Out-Null
if ($LASTEXITCODE) { throw 'Could not make the validation run private' }
$node = (Get-Command node.exe -ErrorAction Stop).Source
$start = [Diagnostics.ProcessStartInfo]::new($node)
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.WorkingDirectory = $repo
foreach ($arg in @((Join-Path $PSScriptRoot 'verify-local.mjs'), $Mode, $runRoot, $ToolRoot, $LiveTarget, [string]$Resume.IsPresent, $CompletedAction)) {
    $start.ArgumentList.Add([string]$arg)
}
# The child waits for assignment before it can launch tools. Closing this job
# on normal exit, Ctrl+C or termination also closes all of this run's children.
. (Join-Path $PSScriptRoot 'validation-job.ps1')

$release = Join-Path $runRoot ('launch-' + [guid]::NewGuid().ToString('N'))
$start.Environment['PILOTWEAVE_VALIDATION_RELEASE'] = $release
$process = [Diagnostics.Process]::Start($start)
$job = [IntPtr]::Zero
try {
    $job = [PilotWeaveValidationJob]::Attach($process.Handle)
    [IO.File]::WriteAllText($release, 'ready')
    while (!$process.WaitForExit(500)) { }
    $result = $process.ExitCode
} finally {
    if ($job -ne [IntPtr]::Zero) { [PilotWeaveValidationJob]::CloseHandle($job) | Out-Null }
    elseif (!$process.HasExited) { $process.Kill($true) }
    if (Test-Path -LiteralPath $release) { Remove-Item -LiteralPath $release }
}
Write-Output "Report: $runRoot\report.html"
exit $result
