param(
    [Parameter(Mandatory=$true)][string]$SourceDirectory,
    [Parameter(Mandatory=$true)][string]$WinDivertDirectory,
    [Parameter(Mandatory=$true)][string]$OutputDirectory
)
$ErrorActionPreference='Stop'
$sourceRoot=(Resolve-Path -LiteralPath $SourceDirectory).Path
$driverRoot=(Resolve-Path -LiteralPath $WinDivertDirectory).Path
$outputRoot=[IO.Path]::GetFullPath($OutputDirectory)
$signature=Get-AuthenticodeSignature -LiteralPath (Join-Path $driverRoot 'x64/WinDivert64.sys')
if($signature.Status -ne 'Valid'){throw 'Driver signature is not valid; refusing to prepare component.'}
& "$PSScriptRoot/build-redirect-cli.ps1" -SourceDirectory $sourceRoot -OutputDirectory $outputRoot
$units=@('ProxyBridge.c','pb_util.c','pb_process.c','pb_rules.c','pb_proxy.c','pb_dns.c','pb_socks5.c','pb_http.c','pb_conntrack.c','pb_relay.c','pb_sockmap.c')
$patchedRoot=Join-Path $outputRoot 'domain-sources'
New-Item -ItemType Directory -Force -Path $patchedRoot | Out-Null
Copy-Item -Path (Join-Path $sourceRoot 'Windows/src/*') -Destination $patchedRoot -Recurse -Force
& python "$PSScriptRoot/patch-domain-engine.py" $patchedRoot
if($LASTEXITCODE -ne 0){throw 'Domain engine patch failed.'}
$routingManifest=Join-Path $PSScriptRoot '../routing-engine/Cargo.toml'
& cargo build --manifest-path $routingManifest --release --locked
if($LASTEXITCODE -ne 0){throw 'Routing engine build failed.'}
$routingLibrary=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../routing-engine/target/release/secret_tunnel_routing.lib'))
$sources=$units | ForEach-Object {Join-Path $patchedRoot $_}
Push-Location -LiteralPath $outputRoot
try {
    & cl.exe /nologo /O2 /MT /LD /utf-8 /DPROXYBRIDGE_EXPORTS /D_CRT_SECURE_NO_WARNINGS /D_WINSOCK_DEPRECATED_NO_WARNINGS "/I$driverRoot/include" @sources /link "/LIBPATH:$driverRoot/x64" WinDivert.lib ws2_32.lib iphlpapi.lib userenv.lib bcrypt.lib ntdll.lib $routingLibrary /OUT:ProxyBridgeCore.dll /DYNAMICBASE /NXCOMPAT
    if($LASTEXITCODE -ne 0){throw 'Core compilation failed.'}
    $objects=$units | ForEach-Object {[IO.Path]::GetFileNameWithoutExtension($_)+'.obj'}
    & cl.exe /nologo /O2 /MT /utf-8 /DPROXYBRIDGE_EXPORTS /D_CRT_SECURE_NO_WARNINGS /D_WINSOCK_DEPRECATED_NO_WARNINGS "/I$patchedRoot" "/I$driverRoot/include" "$PSScriptRoot/check-domain-engine.c" @objects /Fe:check-domain-engine.exe /link "/LIBPATH:$driverRoot/x64" WinDivert.lib ws2_32.lib iphlpapi.lib userenv.lib bcrypt.lib ntdll.lib $routingLibrary
    if($LASTEXITCODE -ne 0){throw 'Native test compilation failed.'}
} finally {Pop-Location}
foreach($file in @('WinDivert.dll','WinDivert64.sys')){
    Copy-Item -LiteralPath (Join-Path $driverRoot "x64/$file") -Destination (Join-Path $outputRoot $file)
}
& (Join-Path $outputRoot 'check-domain-engine.exe')
if($LASTEXITCODE -ne 0){throw 'Native domain routing tests failed.'}
Copy-Item -LiteralPath (Join-Path $driverRoot 'LICENSE') -Destination (Join-Path $outputRoot 'LICENSE.WinDivert.txt')
Write-Output 'Component prepared. Driver has not been loaded by this build script.'

Add-Content -LiteralPath (Join-Path $outputRoot 'source-revision.txt') -Value 'domain-regex-v1'
