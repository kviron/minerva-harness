[CmdletBinding(SupportsShouldProcess)]
param(
    [string]$ShortcutName = 'Minerva IDE — временная сборка.lnk'
)

$ErrorActionPreference = 'Stop'
if (!$IsWindows) { throw 'The desktop preview shortcut is supported on Windows only.' }
if ([IO.Path]::GetFileName($ShortcutName) -ne $ShortcutName -or !$ShortcutName.EndsWith('.lnk')) {
    throw 'ShortcutName must be a desktop shortcut filename ending in .lnk.'
}

$repo = Split-Path $PSScriptRoot -Parent
$desktop = [Environment]::GetFolderPath('Desktop')
$link = Join-Path $desktop $ShortcutName
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($link)
$previous = $shortcut.TargetPath
if (!$PSCmdlet.ShouldProcess($link, 'Build, verify and switch the Minerva IDE preview')) { return }

function Assert-PreviewClosed {
    $previewProcesses = Get-CimInstance Win32_Process -Filter "Name='openidea.exe'" |
        Where-Object { $_.ExecutablePath -eq $previous -or $_.ExecutablePath -eq $buildBinary }
    if ($previewProcesses) { throw 'Close the running preview before updating its shortcut.' }
}

Push-Location -LiteralPath $repo
try {
    $metadata = cargo metadata --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Could not locate the Cargo output directory.' }
    $buildBinary = Join-Path ($metadata | ConvertFrom-Json).target_directory 'debug/openidea.exe'
    Assert-PreviewClosed
    $commit = git rev-parse --short HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Could not identify the current commit.' }
    pnpm tauri build --debug --no-bundle
    if ($LASTEXITCODE -ne 0) { throw 'Windows preview build failed.' }
    if ((git rev-parse --short HEAD) -ne $commit) { throw 'The checkout changed during the build.' }
} finally {
    Pop-Location
}

Assert-PreviewClosed
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$preview = Join-Path $env:LOCALAPPDATA "Minerva IDE/Preview/$stamp-$commit"
$null = New-Item -ItemType Directory -Path $preview
$binary = Join-Path $preview 'openidea.exe'
Copy-Item -LiteralPath $buildBinary -Destination $binary
if ((Get-FileHash -LiteralPath $binary).Hash -ne (Get-FileHash -LiteralPath $buildBinary).Hash) {
    throw 'The preview executable differs from the build output.'
}

# A smoke launch uses disposable settings so it cannot migrate the live chat database.
$smokeRoot = Join-Path $env:TEMP ('minerva-preview-check-' + [guid]::NewGuid())
$smokeEnvironment = @{
    OPENIDEA_CONFIG_DIR = Join-Path $smokeRoot 'config'
    OPENIDEA_DATA_DIR = Join-Path $smokeRoot 'data'
    OPENIDEA_LOCAL_DATA_DIR = Join-Path $smokeRoot 'cache'
    OPENIDEA_LOG_DIR = Join-Path $smokeRoot 'logs'
}
$process = Start-Process -FilePath $binary -WorkingDirectory $preview -WindowStyle Hidden -Environment $smokeEnvironment -PassThru
try {
    $deadline = [datetime]::UtcNow.AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 250
        $process.Refresh()
        if ($process.HasExited) { throw 'The new preview exited before opening a window.' }
    } until (($process.MainWindowHandle -ne 0 -and $process.Responding) -or [datetime]::UtcNow -ge $deadline)
    if ($process.MainWindowHandle -eq 0 -or !$process.Responding) {
        throw "The new preview did not open a responsive window. Logs: $smokeRoot"
    }
} finally {
    if (!$process.HasExited) {
        $null = $process.CloseMainWindow()
        if (!$process.WaitForExit(3000)) { $process.Kill() }
    }
}

$shortcut = $shell.CreateShortcut($link)
if ($shortcut.TargetPath -ne $previous) { throw 'The desktop shortcut changed during the build.' }
$shortcut.TargetPath = $binary
$shortcut.WorkingDirectory = $preview
$shortcut.IconLocation = "$binary,0"
$shortcut.Save()
if ($shell.CreateShortcut($link).TargetPath -ne $binary) { throw 'Could not verify the updated desktop shortcut.' }
Write-Host "Preview ready: $binary"
Write-Host "Shortcut: $link"
