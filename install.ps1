# Installe la dernière version d'Onde pour l'utilisateur courant, sans droits d'administrateur.
#   irm https://raw.githubusercontent.com/dontleteuwatchyou/onde/master/install.ps1 | iex
# Pour le retirer (ta musique et tes réglages restent), télécharge ce script puis :
#   powershell -File install.ps1 -Uninstall
param([switch]$Uninstall)
$ErrorActionPreference = 'Stop'

$dir = Join-Path $env:LOCALAPPDATA 'Programs\Onde'
$exe = Join-Path $dir 'onde.exe'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Onde.lnk'

if ($Uninstall) {
    Remove-Item -Force -ErrorAction SilentlyContinue $shortcut
    Remove-Item -Force -Recurse -ErrorAction SilentlyContinue $dir
    Write-Output "Onde est retiré. Ta musique et tes réglages n'ont pas bougé."
    return
}

if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
    throw "Seule la version x86_64 est compilée."
}

$base = 'https://github.com/dontleteuwatchyou/onde/releases/latest/download'
$asset = 'onde-windows-x86_64.exe'
$temporary = Join-Path $env:TEMP "$asset.$PID"
try {
    Write-Output "Téléchargement d'Onde…"
    Invoke-WebRequest -UseBasicParsing "$base/$asset" -OutFile $temporary
    $expected = ((Invoke-WebRequest -UseBasicParsing "$base/$asset.sha256").Content -split '\s+')[0]
    $actual = (Get-FileHash -Algorithm SHA256 $temporary).Hash
    if ($actual -ine $expected) { throw "Le fichier téléchargé est abîmé." }
    New-Item -ItemType Directory -Force $dir | Out-Null
    Move-Item -Force $temporary $exe
} finally {
    Remove-Item -Force -ErrorAction SilentlyContinue $temporary
}

$link = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
$link.TargetPath = $exe
$link.Save()

Write-Output "Onde est installé. Il est dans ton menu Démarrer."
if (-not (Get-Command yt-dlp -ErrorAction SilentlyContinue)) {
    Write-Output "En option : installe yt-dlp et ffmpeg pour télécharger depuis YouTube et SoundCloud."
}
