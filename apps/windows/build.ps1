<#
.SYNOPSIS
  Build the native Windows app: a WinUI 3 interface written in Rust
  (windows-reactor) on the Rust engine, as one self-contained folder.

.EXAMPLE
  apps\windows\build.ps1                 # this PC's architecture, app folder only
  apps\windows\build.ps1 -Installer      # also the setup .exe (needs Inno Setup 6)
  apps\windows\build.ps1 -Arch arm64

.NOTES
  Needs Rust (rustup) with the MSVC toolchain. The first build downloads the
  Windows App SDK runtime it bundles. Run from anywhere in the repository.
#>
param(
    [ValidateSet('x64', 'arm64')][string]$Arch = $(if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }),
    [switch]$Installer
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path "$PSScriptRoot\..\..").Path
Set-Location $root

function Step($text) { Write-Host "`n==> $text" -ForegroundColor Blue }
function Check($what) { if ($LASTEXITCODE -ne 0) { throw "$what failed ($LASTEXITCODE)" } }

$version = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value
$target = if ($Arch -eq 'arm64') { 'aarch64-pc-windows-msvc' } else { 'x86_64-pc-windows-msvc' }
$out = "$root\dist\windows-native\$Arch"
$app = "$out\app"
# its own target folder, so the staged runtime isn't mixed with other builds
$targetDir = "$root\target\windows-app"
$profileDir = "$targetDir\$target\release"

if (-not (Test-Path "$root\extension\dist\chromium")) {
    $bash = Get-Command bash -ErrorAction SilentlyContinue
    if ($bash) {
        Step 'Browser extension'
        & $bash.Source extension/build.sh
        Check 'extension/build.sh'
    } else {
        Write-Warning 'bash not found: the browser extension is not bundled (run extension/build.sh)'
    }
}

Step "Windows app ($target)"
rustup target add $target | Out-Null
cargo build --release -p odm-windows --target $target --target-dir $targetDir
Check 'cargo build'
if (-not (Test-Path "$profileDir\OpenDownloadManager.exe")) { throw "missing $profileDir\OpenDownloadManager.exe" }

Step 'Assembling the app folder'
if (Test-Path $app) { Remove-Item -Recurse -Force $app }
New-Item -ItemType Directory -Force $app | Out-Null
# the executable and the Windows App SDK runtime staged beside it; not
# Cargo's own working folders and files
$skipDirs = @('deps', 'build', 'incremental', 'examples', '.fingerprint')
Get-ChildItem $profileDir -Force | ForEach-Object {
    if ($_.PSIsContainer) {
        if ($skipDirs -notcontains $_.Name) { Copy-Item $_.FullName "$app\$($_.Name)" -Recurse }
    } elseif ($_.Extension -notin @('.d', '.pdb', '.rlib', '.lib', '.exp') -and $_.Name -ne '.cargo-lock') {
        Copy-Item $_.FullName $app
    }
}
New-Item -ItemType Directory -Force "$app\Assets\tray" | Out-Null
Copy-Item src-tauri\icons\icon.ico "$app\Assets\icon.ico"
Copy-Item download-manager-ui\brand\dist\tray\windows-linux\*.ico "$app\Assets\tray\"
if (Test-Path extension\dist\chromium) {
    New-Item -ItemType Directory -Force "$app\extension" | Out-Null
    Copy-Item extension\dist\chromium, extension\dist\firefox "$app\extension\" -Recurse
}
Get-ChildItem $app | Select-Object Name, Length | Format-Table -AutoSize | Out-String | Write-Host

if ($Installer) {
    Step 'Installer'
    $iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
    if (-not $iscc) {
        $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
    }
    if (-not $iscc) { throw 'Inno Setup 6 not found (winget install JRSoftware.InnoSetup)' }
    & $iscc "/DAppVersion=$version" "/DArch=$Arch" "/DSourceDir=$app" "/DOutputDir=$root\dist\windows-native" apps\windows\installer\odm.iss
    Check 'iscc'
}

Step "Done: $app"
