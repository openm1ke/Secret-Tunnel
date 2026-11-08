param(
    [string]$SdkDirectory = '',
    [string]$JavaDirectory = 'D:/SDK/LinuxSDK/amazon-corretto-17.0.12.7.1-linux-x64'
)
$ErrorActionPreference='Stop'
if(-not $SdkDirectory){
    if(Test-Path -LiteralPath 'D:/SecretTunnelBuild/android-sdk-view'){$SdkDirectory='D:/SecretTunnelBuild/android-sdk-view'}
    else {$SdkDirectory='D:/SDK/AndroidSDK'}
}
$repo=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
function LinuxPath([string]$value) {
    $result=& wsl.exe -e wslpath -a $value
    if($LASTEXITCODE -ne 0){throw "Cannot translate path: $value"}
    return $result.Trim()
}
$script=LinuxPath (Join-Path $repo 'scripts/build-android.sh')
$sdk=LinuxPath $SdkDirectory
$java=LinuxPath $JavaDirectory
& wsl.exe -e env "ANDROID_HOME=$sdk" "JAVA_HOME=$java" bash -c 'export PATH="$HOME/.cargo/bin:$PATH"; exec bash "$1"' build-android $script
if($LASTEXITCODE -ne 0){throw 'Android build failed; see the output above.'}
