$ErrorActionPreference = 'Stop'

try {
    $old = Get-Process -Id ([int]$env:ST_UPDATE_PID) -ErrorAction SilentlyContinue
    if ($old -and !$old.WaitForExit(60000)) {
        throw 'Приложение не закрылось за 60 секунд.'
    }

    $setup = Start-Process -FilePath $env:ST_UPDATE_INSTALLER -ArgumentList '/S' -PassThru
    if (!$setup.WaitForExit(1800000)) {
        throw 'Установщик не завершился за 30 минут.'
    }
    if ($setup.ExitCode -ne 0) {
        throw "Установщик завершился с кодом $($setup.ExitCode)."
    }
    Start-Process -FilePath $env:ST_UPDATE_APP -WorkingDirectory (Split-Path -Parent $env:ST_UPDATE_APP)
    Remove-Item -LiteralPath $env:ST_UPDATE_INSTALLER -ErrorAction SilentlyContinue
} catch {
    $message = $_.Exception.Message
    $message | Set-Content -LiteralPath $env:ST_UPDATE_LOG -Encoding UTF8
    Add-Type -AssemblyName System.Windows.Forms
    [void][Windows.Forms.MessageBox]::Show($message, 'Обновление Secret Tunnel')
    exit 1
}
