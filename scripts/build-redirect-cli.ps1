param(
    [Parameter(Mandatory=$true)][string]$SourceDirectory,
    [Parameter(Mandatory=$true)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
# Build only the userspace CLI. This script never downloads/loads a driver,
# installs a service, changes security settings, or edits upstream sources.
$sourceRoot = (Resolve-Path -LiteralPath $SourceDirectory).Path
$revision = & git -C $sourceRoot rev-parse HEAD
if ($revision -ne 'cb897b017a88a142ec803c3af138b0ad75c2ae12') {
    throw 'Review the adapter against this ProxyBridge revision before building.'
}
$outputRoot = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $outputRoot | Out-Null
$source = Get-Content -LiteralPath (Join-Path $sourceRoot 'Windows/cli/main.c') -Raw
$source = $source.Replace("`r`n", "`n")
$original = "    while (InterlockedCompareExchange(&g_running, 0, 0) != 0)`n        Sleep(200);"
if (-not $source.Contains($original)) { throw 'Upstream CLI lifecycle changed.' }
$replacement = @'
    fflush(stdout);
    while (InterlockedCompareExchange(&g_running, 0, 0) != 0) {
        HANDLE input = GetStdHandle(STD_INPUT_HANDLE);
        DWORD available = 0;
        BOOL pipe_ok = PeekNamedPipe(input, NULL, 0, NULL, &available, NULL);
        DWORD pipe_error = pipe_ok ? ERROR_SUCCESS : GetLastError();
        // A byte from the parent requests stop; EOF also stops on parent exit.
        if ((pipe_ok && available > 0) || pipe_error == ERROR_BROKEN_PIPE) {
            if (InterlockedExchange(&g_running, 0) && g_Stop) g_Stop();
            break;
        }
        Sleep(100);
    }
'@
$source = $source.Replace($original, $replacement)
# A 253-byte DNS name encoded as hex plus telemetry headers exceeds 512 bytes.
$source = $source.Replace('#define OUTQ_LEN   512', '#define OUTQ_LEN   1024')
# Never start interception with only a subset of the user's routing rules.
$failed="            fail++;`n            continue;"
if(-not $source.Contains($failed)){throw 'Upstream rule failure path changed.'}
$source=$source.Replace($failed,'            return 1;')

$patched = Join-Path $outputRoot 'secret-tunnel-proxybridge-cli.c'
Set-Content -LiteralPath $patched -Value $source -Encoding utf8
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$vsRoot = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vsRoot) { throw 'Install Microsoft C++ Build Tools first.' }
$vcvars = Join-Path $vsRoot 'VC/Auxiliary/Build/vcvarsall.bat'
$compilerEnvironment = & cmd.exe /d /s /c "`"$vcvars`" x64 >nul && set"
if ($LASTEXITCODE -ne 0) { throw 'Cannot initialise MSVC environment.' }
foreach ($entry in $compilerEnvironment) {
    if ($entry -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process') }
}
Push-Location -LiteralPath $outputRoot
try {
    & cl.exe /nologo /O2 /MT /W3 /utf-8 /D_CRT_SECURE_NO_WARNINGS $patched /Fe:ProxyBridge_CLI.exe /link /DYNAMICBASE /NXCOMPAT
    if ($LASTEXITCODE -ne 0) { throw 'CLI compilation failed.' }
} finally { Pop-Location }
Copy-Item -LiteralPath (Join-Path $sourceRoot 'LICENSE') -Destination (Join-Path $outputRoot 'LICENSE.ProxyBridge.txt')
Set-Content -LiteralPath (Join-Path $outputRoot 'source-revision.txt') -Value "$revision`nSecret Tunnel patch: stdin stop + parent EOF cleanup."
Write-Output "Built $outputRoot/ProxyBridge_CLI.exe. Core DLL and reviewed signed driver are separate prerequisites."
