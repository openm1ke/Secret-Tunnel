$ErrorActionPreference='SilentlyContinue'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$OutputEncoding=[Console]::OutputEncoding
$found=@{}
function Add-App([string]$name,[string]$target,[string[]]$patterns,[string]$source) {
    $target=[Environment]::ExpandEnvironmentVariables($target).Trim('"')
    if($target -notmatch '^[A-Za-z]:[\\/]' -or -not $target.EndsWith('.exe',[StringComparison]::OrdinalIgnoreCase)){return}
    if(-not(Test-Path -LiteralPath $target -PathType Leaf)){return}
    $base=[IO.Path]::GetFileName($target)
    if($base -match '^(unins.*|uninstall|setup|rundll32|cmd|powershell|pwsh|msiexec|explorer|ssh|ProxyBridge.*|SecretTunnel.*|secret-tunnel.*)\.exe$'){return}
    if($name -match '(?i)uninstall|удалени|справка|\bhelp\b|repair'){return}
    if(-not $name){$name=[IO.Path]::GetFileNameWithoutExtension($target)}
    $directory=Split-Path $target
    if(-not $patterns -or $patterns.Count -eq 0){$patterns=@($target)}
    foreach($companion in @('Updater.exe','updater.exe')){
        $candidate=Join-Path $directory $companion
        if((Test-Path -LiteralPath $candidate -PathType Leaf) -and $candidate -ne $target){$patterns+= $candidate}
    }
    $key=$target.ToLowerInvariant()
    foreach($channel in @(@('Discord','Discord.exe'),@('DiscordCanary','DiscordCanary.exe'),@('DiscordPTB','DiscordPTB.exe'))){
        $discordRoot=Join-Path $env:LOCALAPPDATA $channel[0]
        if($target.StartsWith($discordRoot+'\',[StringComparison]::OrdinalIgnoreCase) -and $base -ieq $channel[1]){
            $key='discord:'+$channel[0].ToLowerInvariant();$name=$channel[0];$patterns=@($channel[1])
            $updater=Join-Path $discordRoot 'Update.exe';if(Test-Path -LiteralPath $updater -PathType Leaf){$patterns+=$updater}
            $launcher=Join-Path $discordRoot $channel[1];if(Test-Path -LiteralPath $launcher -PathType Leaf){$target=$launcher}
        }
    }
    if($base -in @('firefox.exe','private_browsing.exe') -and (Test-Path -LiteralPath (Join-Path $directory 'firefox.exe') -PathType Leaf)){
        $key='firefox:'+$directory.ToLowerInvariant();$name='Firefox';$target=Join-Path $directory 'firefox.exe';$patterns=@($target)
        foreach($companion in @('private_browsing.exe','updater.exe')){ $candidate=Join-Path $directory $companion;if(Test-Path -LiteralPath $candidate -PathType Leaf){$patterns+=$candidate} }
    }
    if(-not $found.ContainsKey($key)){
        $found[$key]=[pscustomobject]@{id=$key;name=$name;executable=$target;patterns=@($patterns | Sort-Object -Unique);source=$source}
    }
}

# Version-independent Discord selection includes its Squirrel updater.
foreach($channel in @(@('Discord','Discord.exe'),@('DiscordCanary','DiscordCanary.exe'),@('DiscordPTB','DiscordPTB.exe'))){
    $root=Join-Path $env:LOCALAPPDATA $channel[0]
    $main=Get-ChildItem -LiteralPath $root -Directory -Filter 'app-*' | Sort-Object Name -Descending | ForEach-Object {Join-Path $_.FullName $channel[1]} | Where-Object {Test-Path -LiteralPath $_ -PathType Leaf} | Select-Object -First 1
    if($main){
        $patterns=@($channel[1]);$updater=Join-Path $root 'Update.exe'
        if(Test-Path -LiteralPath $updater -PathType Leaf){$patterns+=$updater}
        Add-App $channel[0] $main $patterns 'installed'
    }
}
foreach($telegramRoot in @((Join-Path $env:APPDATA 'Telegram Desktop'),(Join-Path $env:LOCALAPPDATA 'Telegram Desktop'))){
    Add-App 'Telegram' (Join-Path $telegramRoot 'Telegram.exe') @() 'installed'
}

$shell=New-Object -ComObject WScript.Shell
$menus=@([Environment]::GetFolderPath('Programs'),[Environment]::GetFolderPath('CommonPrograms'))
foreach($menu in $menus){
    Get-ChildItem -LiteralPath $menu -Filter '*.lnk' -Recurse -File | Select-Object -First 3000 | ForEach-Object {
        $link=$shell.CreateShortcut($_.FullName)
        $target=$link.TargetPath
        if([IO.Path]::GetFileName($target) -ieq 'Update.exe' -and $link.Arguments -match '--processStart\s+"?([\w. -]+\.exe)"?'){
            $image=$Matches[1];$root=Split-Path $target
            $main=Get-ChildItem -LiteralPath $root -Directory -Filter 'app-*' | Sort-Object Name -Descending | ForEach-Object {Join-Path $_.FullName $image} | Where-Object {Test-Path -LiteralPath $_ -PathType Leaf} | Select-Object -First 1
            if($main){Add-App $_.BaseName $main @($image,$target) 'installed'}
        }else{Add-App $_.BaseName $target @() 'installed'}
    }
}

# Read uninstall metadata, without running any registered commands.
foreach($registry in @('HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*','HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*','HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*')){
    Get-ItemProperty $registry | Select-Object -First 3000 | ForEach-Object {
        if($_.DisplayName -and $_.DisplayIcon -match '^\s*"?(.+?\.exe)"?(?:,.*)?$'){
            Add-App $_.DisplayName $Matches[1] @() 'installed'
        }
    }
}

Get-Process | Where-Object {$_.Path -and $_.MainWindowHandle -ne 0} | ForEach-Object {
    $target=$_.Path
    $name=$_.FileVersionInfo.ProductName
    if(-not $name){$name=$_.ProcessName}
    Add-App $name $target @() 'running'
}
$apps=@($found.Values | Sort-Object name,executable)
ConvertTo-Json -InputObject $apps -Depth 6 -Compress
