# Start a session, type a command into it, read the screen, end the session (Windows PowerShell or PowerShell 7).
$ErrorActionPreference = 'Stop'

$name = 'drive-demo'
$sock = Join-Path $env:LOCALAPPDATA "vtee\$name.sock"

Start-Process vtee -WindowStyle Hidden `
    -ArgumentList '--headless', '--name', $name, '--writable', '--', 'cmd'

$tries = 0
while (-not ((vtee ls 2>$null) -match "^$name\s.*live")) {
    if (++$tries -gt 50) { throw 'session did not start' }
    Start-Sleep -Milliseconds 200
}

# DO^NE is typed, DONE is output: in cmd, ^ escapes the N, so waiting for DONE cannot match the echo.
# The commands go in through a file so PowerShell 5.1 does not put a byte-order mark in front of them.
$commands = Join-Path $env:TEMP 'vtee-drive.ndjson'
Set-Content -Path $commands -Encoding ascii -Value @(
    '{"cmd":"send","text":"echo DO^NE\r"}',
    '{"cmd":"wait_for","text":"DONE","timeout_ms":5000}'
)
cmd /c "vtee --connect `"$sock`" --text < `"$commands`""

vtee kill $name
Remove-Item $commands
