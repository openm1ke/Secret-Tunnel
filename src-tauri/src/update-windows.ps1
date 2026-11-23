$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$parentExited = $false
$utf8 = New-Object System.Text.UTF8Encoding($false)

function Write-UpdateLog([string]$message) {
    [IO.File]::AppendAllText($env:ST_UPDATE_LOG, ([DateTime]::UtcNow.ToString('o') + ' ' + $message + [Environment]::NewLine), $utf8)
}

try {
    if (!(Test-Path -LiteralPath $env:ST_UPDATE_INSTALLER -PathType Leaf)) { throw 'Не найден скачанный установщик.' }
    if (!(Test-Path -LiteralPath $env:ST_UPDATE_APP -PathType Leaf)) { throw 'Не найдено установленное приложение.' }
    if ((Get-FileHash -LiteralPath $env:ST_UPDATE_INSTALLER -Algorithm SHA256).Hash -ne $env:ST_UPDATE_SHA256) {
        throw 'Контрольная сумма установщика изменилась. Скачайте обновление заново.'
    }
    # Retain the exact process handle before acknowledging; a recycled PID is not the old app.
    $old = [Diagnostics.Process]::GetProcessById([int]$env:ST_UPDATE_PID)
    $null = $old.Handle
    Write-UpdateLog 'helper ready'
    [IO.File]::WriteAllText($env:ST_UPDATE_READY, 'ready', [Text.Encoding]::ASCII)
    if (!$old.WaitForExit(60000)) {
        throw 'Приложение не закрылось за 60 секунд.'
    }
    $parentExited = $true
    $old.Dispose()
    $directory = Split-Path -Parent $env:ST_UPDATE_APP
    # NSIS consumes everything after /D= as the directory, including spaces: keep it last and unquoted.
    $arguments = '/P /UPDATE /D=' + $directory
    Write-UpdateLog 'starting installer'
    $setup = Start-Process -FilePath $env:ST_UPDATE_INSTALLER -ArgumentList $arguments -WorkingDirectory (Split-Path -Parent $env:ST_UPDATE_INSTALLER) -WindowStyle Normal -PassThru
    # Windows PowerShell otherwise may lose the exit code when a short-lived process exits.
    $null = $setup.Handle

    if (!$setup.WaitForExit(1800000)) {
        throw 'Установщик не завершился за 30 минут.'
    }
    $exitCode = $setup.ExitCode
    $setup.Dispose()
    if ($null -eq $exitCode -or $exitCode -ne 0) {
        throw "Установщик завершился с кодом $exitCode."
    }
    Write-UpdateLog 'installer completed; restarting app'
    Start-Process -FilePath $env:ST_UPDATE_APP -WorkingDirectory $directory
    Remove-Item -LiteralPath $env:ST_UPDATE_INSTALLER -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $env:ST_UPDATE_ERROR -ErrorAction SilentlyContinue
    Write-UpdateLog 'update completed'
} catch {
    $message = 'Не удалось установить обновление: ' + $_.Exception.Message
    try {
        [IO.File]::WriteAllText($env:ST_UPDATE_ERROR, $message, $utf8)
        Write-UpdateLog $message
    } catch { [Console]::Error.WriteLine($message) }
    if ($parentExited) {
        try {
            # Report the durable failure in the app instead of leaving the user with no window.
            Start-Process -FilePath $env:ST_UPDATE_APP -WorkingDirectory (Split-Path -Parent $env:ST_UPDATE_APP)
        } catch {
            Add-Type -AssemblyName System.Windows.Forms
            [void][Windows.Forms.MessageBox]::Show($message + [Environment]::NewLine + 'Журнал: ' + $env:ST_UPDATE_LOG, 'Обновление Secret Tunnel')
        }
    }
    exit 1
}
