param([string]$WorkDirectory = (Join-Path $env:TEMP 'secret-tunnel-release-tools'))
$ErrorActionPreference='Stop'
$repo=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
New-Item -ItemType Directory -Force -Path $WorkDirectory | Out-Null
$source=Join-Path $WorkDirectory 'ProxyBridge'
if(-not(Test-Path -LiteralPath (Join-Path $source '.git'))){
    & git clone --no-checkout https://github.com/InterceptSuite/ProxyBridge.git $source
    if($LASTEXITCODE -ne 0){throw 'ProxyBridge clone failed'}
    & git -C $source checkout --detach cb897b017a88a142ec803c3af138b0ad75c2ae12
    if($LASTEXITCODE -ne 0){throw 'ProxyBridge revision checkout failed'}
}
$driver=Join-Path $WorkDirectory 'WinDivert-2.2.2-A'
if(-not(Test-Path -LiteralPath (Join-Path $driver 'x64/WinDivert64.sys'))){
    $archive=Join-Path $WorkDirectory 'WinDivert-2.2.2-A.zip'
    Invoke-WebRequest https://github.com/basil00/WinDivert/releases/download/v2.2.2/WinDivert-2.2.2-A.zip -OutFile $archive
    Expand-Archive -LiteralPath $archive -DestinationPath $WorkDirectory -Force
}
$pins=@{
    'x64/WinDivert64.sys'='8DA085332782708D8767BCACE5327A6EC7283C17CFB85E40B03CD2323A90DDC2'
    'x64/WinDivert.dll'='C1E060EE19444A259B2162F8AF0F3FE8C4428A1C6F694DCE20DE194AC8D7D9A2'
    'x64/WinDivert.lib'='C5678D544EB0121A189D1139F54E0C67854DC64D1C897111A27EF2E52CB38EB3'
    'include/windivert.h'='5017A1768C1592FD664C0C2D3D2D30F81FAD4AB98D322B1914C6F0A33FCACDF9'
}
foreach($file in $pins.Keys){
    if((Get-FileHash -LiteralPath (Join-Path $driver $file) -Algorithm SHA256).Hash -ne $pins[$file]){throw "Upstream integrity check failed: $file"}
}
$output=Join-Path $WorkDirectory 'component-build'
& "$PSScriptRoot/build-redirect-core.ps1" -SourceDirectory $source -WinDivertDirectory $driver -OutputDirectory $output
$destination=Join-Path $repo 'src-tauri/resources/proxybridge'
New-Item -ItemType Directory -Force -Path $destination | Out-Null
foreach($file in @('ProxyBridge_CLI.exe','ProxyBridgeCore.dll','WinDivert.dll','WinDivert64.sys','LICENSE.ProxyBridge.txt','LICENSE.WinDivert.txt','source-revision.txt')){
    Copy-Item -LiteralPath (Join-Path $output $file) -Destination (Join-Path $destination $file)
}
