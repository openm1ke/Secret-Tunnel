param([string]$Version='0.2.0')
$ErrorActionPreference='Stop'
$repo=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$stage=Join-Path $repo 'artifacts/windows-stage'
$release=Join-Path $repo 'artifacts/release'
New-Item -ItemType Directory -Force -Path $stage,$release | Out-Null
Copy-Item -LiteralPath (Join-Path $repo 'src-tauri/target/release/secret-tunnel.exe') -Destination (Join-Path $stage 'SecretTunnel.exe')
foreach($folder in @('proxybridge','server','licenses')){
    $destination=if($folder -eq 'proxybridge'){Join-Path $stage 'tools/proxybridge'}else{Join-Path $stage $folder}
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    Copy-Item -Path (Join-Path $repo "src-tauri/resources/$folder/*") -Destination $destination -Recurse -Force
}
Copy-Item -LiteralPath (Join-Path $repo 'docs/WINDOWS-RELEASE.md') -Destination (Join-Path $stage 'README-Windows.md')
Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination (Join-Path $stage 'LICENSE.txt')
$required=@('SecretTunnel.exe','tools/proxybridge/ProxyBridge_CLI.exe','tools/proxybridge/ProxyBridgeCore.dll','tools/proxybridge/WinDivert.dll','tools/proxybridge/WinDivert64.sys','server/secret-tunnel-agent-linux-x86_64','README-Windows.md')
foreach($file in $required){if(-not(Test-Path -LiteralPath (Join-Path $stage $file))){throw "Missing release runtime: $file"}}
if(-not (Get-Content -Raw (Join-Path $stage 'tools/proxybridge/source-revision.txt')).Contains('domain-regex-v1')){throw 'Missing domain engine patch'}
$manifest=Get-ChildItem -LiteralPath $stage -File -Recurse | Where-Object {$_.FullName -ne (Join-Path $stage 'SHA256SUMS.txt')} | Sort-Object FullName | ForEach-Object {
    "$((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLower())  $([IO.Path]::GetRelativePath($stage,$_.FullName).Replace('\','/'))"
}
$manifest | Set-Content -Encoding utf8 (Join-Path $stage 'SHA256SUMS.txt')
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath (Join-Path $release "SecretTunnel-$Version-windows-x64-portable.zip") -Force
$installers=@(Get-ChildItem -Path (Join-Path $repo 'src-tauri/target/release/bundle/nsis/*.exe') | Where-Object {$_.Name.Contains("_${Version}_")})
if($installers.Count -ne 1){throw "Expected one NSIS installer for version $Version; found $($installers.Count)"}
$installer=$installers[0]
Copy-Item -LiteralPath $installer.FullName -Destination (Join-Path $release "SecretTunnel-$Version-windows-x64-setup.exe")
