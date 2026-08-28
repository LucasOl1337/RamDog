# RamDog — instalador de um comando (Windows x64)
# irm https://raw.githubusercontent.com/LucasOl1337/RamDog/main/install.ps1 | iex
#
# Não precisa de Rust, cargo, git nem admin. Copia o release pra
# %LOCALAPPDATA%\RamDog, coloca no PATH do usuário e cria atalho
# (Desktop + Menu Iniciar) que abre elevado — temperatura da CPU.

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

if (-not [Environment]::Is64BitOperatingSystem) {
    throw "RamDog é Windows x64."
}

$repo = 'LucasOl1337/RamDog'
$dest = Join-Path $env:LOCALAPPDATA 'RamDog'
$assetName = 'RamDog-windows-x64.zip'

Write-Host "RamDog — baixando o release mais recente..."

$release = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest" -Headers @{
    'User-Agent' = 'RamDog-install'
    'Accept'     = 'application/vnd.github+json'
}
$asset = $release.assets | Where-Object { $_.name -eq $assetName } | Select-Object -First 1
if (-not $asset) {
    throw "Release $($release.tag_name) não tem $assetName."
}

$tmp = Join-Path $env:TEMP "RamDog-$($release.tag_name)"
$zip = Join-Path $env:TEMP $assetName
if (Test-Path $tmp) { Remove-Item $tmp -Recurse -Force }
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

Write-Host "  $($release.tag_name)  $($asset.browser_download_url)"
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zip -UseBasicParsing
Expand-Archive -Path $zip -DestinationPath $tmp -Force

$exe = Get-ChildItem $tmp -Filter 'ramdog.exe' -Recurse | Select-Object -First 1
if (-not $exe) { throw "ramdog.exe não veio no zip." }

New-Item -ItemType Directory -Force -Path $dest | Out-Null
Get-ChildItem $tmp -Recurse -File | ForEach-Object {
    Copy-Item $_.FullName (Join-Path $dest $_.Name) -Force
}
Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item $zip -Force -ErrorAction SilentlyContinue

function Add-RunAsShortcut([string]$path, [string]$target) {
    $ws = New-Object -ComObject WScript.Shell
    $sc = $ws.CreateShortcut($path)
    $sc.TargetPath = $target
    $sc.WorkingDirectory = [IO.Path]::GetDirectoryName($target)
    $ico = Join-Path $dest 'ramdog.ico'
    if (Test-Path $ico) { $sc.IconLocation = $ico } else { $sc.IconLocation = $target }
    $sc.Description = 'RamDog'
    $sc.Save()
    $bytes = [IO.File]::ReadAllBytes($path)
    $bytes[0x15] = $bytes[0x15] -bor 0x20
    [IO.File]::WriteAllBytes($path, $bytes)
}

$target = Join-Path $dest 'ramdog.exe'
$startDir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
New-Item -ItemType Directory -Force -Path $startDir | Out-Null
Add-RunAsShortcut (Join-Path $startDir 'RamDog.lnk') $target
Add-RunAsShortcut (Join-Path ([Environment]::GetFolderPath('Desktop')) 'RamDog.lnk') $target

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not $userPath) { $userPath = '' }
$parts = $userPath -split ';' | Where-Object { $_ -and $_.Trim() -ne '' }
if ($parts -notcontains $dest) {
    $newPath = ($parts + $dest) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
}
if ($env:Path -notlike "*$dest*") {
    $env:Path = $dest + ';' + $env:Path
}

Write-Host ""
Write-Host "Instalado em $dest  ($($release.tag_name))"
Write-Host "Abrir:  ramdog    ou o atalho RamDog no Desktop (o RamDog pede elevacao ao abrir)"
Write-Host ""

$dotnet = Get-Command dotnet -ErrorAction SilentlyContinue
if (-not $dotnet) {
    Write-Host "Opcional: .NET 8 runtime para temperatura de CPU/RAM (helper hwtemp)."
    Write-Host "  https://dotnet.microsoft.com/download/dotnet/8.0"
}

Start-Process $target
