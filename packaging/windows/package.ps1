<#
.SYNOPSIS
  Build, sign and package CraftRadio for Windows.

.DESCRIPTION
  The script writes two files to $env:DIST. The default folder is dist/release.
  craftradio-<version>-windows-<arch>.msi is the per-machine installer (WiX v5).
  craftradio-<version>-windows-<arch>-portable.zip holds the same files without an installer.

  The programs link the C runtime statically, so they do not need the Visual C++ redistributable.
  LAME is the separate DLL libmp3lame.dll from vcpkg (docs/mp3-release.md).
  A custom vcpkg triplet also links its C runtime statically.
  sign.ps1 signs the files. Without signing secrets, it gives a warning and signs nothing.

  The script needs Rust (MSVC), the Windows SDK, vcpkg and WiX v5 with the UI and Util extensions.
  Without VCPKG_ROOT or VCPKG_INSTALLATION_ROOT, the script clones vcpkg into the target folder.

.EXAMPLE
  pwsh packaging/windows/package.ps1 -Arch x64
  pwsh packaging/windows/package.ps1 -Arch arm64 -SkipBuild
#>
param(
  [ValidateSet('x64', 'arm64')] [string] $Arch = 'x64',
  [switch] $SkipBuild
)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$AppId = 'io.github.radioterminal.craftradio'
$LameVersion = '3.100'

function Invoke-Native([string] $What, [scriptblock] $Block) {
  Write-Output "==> $What"
  & $Block
  if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

# The root Cargo.toml has the only copy of the version. CRAFTRADIO_VERSION overrides it.
$Version = $env:CRAFTRADIO_VERSION
if (-not $Version) {
  $inPkg = $false
  foreach ($line in Get-Content (Join-Path $Root 'Cargo.toml')) {
    if ($line -match '^\s*\[') { $inPkg = ($line.Trim() -eq '[workspace.package]'); continue }
    if ($inPkg -and $line -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
  }
}
if (-not $Version) { throw 'could not read [workspace.package] version from Cargo.toml' }
# The MSI ProductVersion must be numeric. It does not keep a pre-release tag.
$MsiVersion = ($Version -split '-')[0]

$Target = switch ($Arch) { 'x64' { 'x86_64-pc-windows-msvc' } 'arm64' { 'aarch64-pc-windows-msvc' } }
$Dist = if ($env:DIST) { $env:DIST } else { Join-Path $Root 'dist\release' }
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

if (-not $env:CRAFTRADIO_BUILD_SHA) { $env:CRAFTRADIO_BUILD_SHA = (git -C $Root rev-parse HEAD 2>$null) }
if (-not $env:CRAFTRADIO_BUILD_DATE) { $env:CRAFTRADIO_BUILD_DATE = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd') }

Write-Output "CraftRadio $Version for Windows $Arch ($Target)"

# ---- LAME --------------------------------------------------------------------------------------
# The triplet makes a DLL that links the C runtime statically, as the programs do.
$Triplet = "$Arch-windows-static-crt"
$TripletDir = Join-Path $TargetDir 'vcpkg-triplets'
New-Item -ItemType Directory -Force -Path $TripletDir | Out-Null
Set-Content -Path (Join-Path $TripletDir "$Triplet.cmake") -Value @"
set(VCPKG_TARGET_ARCHITECTURE $Arch)
set(VCPKG_CRT_LINKAGE static)
set(VCPKG_LIBRARY_LINKAGE dynamic)
"@
$Vcpkg = if ($env:VCPKG_ROOT) { $env:VCPKG_ROOT } elseif ($env:VCPKG_INSTALLATION_ROOT) { $env:VCPKG_INSTALLATION_ROOT } else { Join-Path $TargetDir 'vcpkg' }
if (-not (Test-Path (Join-Path $Vcpkg 'vcpkg.exe'))) {
  if (-not (Test-Path (Join-Path $Vcpkg 'bootstrap-vcpkg.bat'))) {
    Invoke-Native 'clone vcpkg' { git clone --depth 1 https://github.com/microsoft/vcpkg $Vcpkg }
  }
  Invoke-Native 'bootstrap vcpkg' { & (Join-Path $Vcpkg 'bootstrap-vcpkg.bat') -disableMetrics }
}
$LameRoot = Join-Path $TargetDir "vcpkg-installed"
Invoke-Native "vcpkg install mp3lame:$Triplet" {
  & (Join-Path $Vcpkg 'vcpkg.exe') install "mp3lame:$Triplet" --overlay-triplets=$TripletDir --x-install-root=$LameRoot --disable-metrics
}
$LamePrefix = Join-Path $LameRoot $Triplet
$LameDll = Join-Path $LamePrefix 'bin\libmp3lame.dll'
if (-not (Test-Path $LameDll)) { throw "vcpkg did not write $LameDll" }

if (-not $SkipBuild) {
  # The static C runtime applies to the target only. Build scripts and proc-macros keep the default.
  $flagVar = 'CARGO_TARGET_' + ($Target.ToUpper() -replace '-', '_') + '_RUSTFLAGS'
  [Environment]::SetEnvironmentVariable($flagVar, '-C target-feature=+crt-static')
  # ffi/lame links libmp3lame.lib from this folder.
  $env:LAME_LIB_DIR = Join-Path $LamePrefix 'lib'
  $env:LAME_LIB_NAME = 'libmp3lame'
  Invoke-Native "cargo build ($Target)" { cargo build --release --locked -p radio -p radio-cli --target $Target }
}

$Bin = Join-Path $TargetDir "$Target\release"

# Check the PE headers before packaging. The machine must match -Arch.
# Both programs use subsystem 3 (console), so the output of `craftradio --headless` is visible.
# Thus a console window opens with the app (docs/releasing.md, "Known gaps").
function Get-PeHeader([string] $Path) {
  $bytes = [System.IO.File]::ReadAllBytes($Path)
  $pe = [BitConverter]::ToInt32($bytes, 0x3C)
  return @{ Machine = [BitConverter]::ToUInt16($bytes, $pe + 4); Subsystem = [BitConverter]::ToUInt16($bytes, $pe + 0x5C) }
}
$Machine = switch ($Arch) { 'x64' { 0x8664 } 'arm64' { 0xAA64 } }
foreach ($check in @(@('craftradio.exe', 3), @('craftradio-cli.exe', 3))) {
  $h = Get-PeHeader (Join-Path $Bin $check[0])
  if ($h.Machine -ne $Machine) { throw "$($check[0]) is for machine 0x$('{0:X}' -f $h.Machine), expected 0x$('{0:X}' -f $Machine) ($Arch)" }
  if ($h.Subsystem -ne $check[1]) { throw "$($check[0]) has PE subsystem $($h.Subsystem), expected $($check[1])" }
  Write-Output "ok $($check[0]): $Arch, PE subsystem $($h.Subsystem)"
}
$Stage = Join-Path $TargetDir "windows-package\$Arch"
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Stage | Out-Null
foreach ($exe in 'craftradio.exe', 'craftradio-cli.exe') { Copy-Item (Join-Path $Bin $exe) $Stage }
Copy-Item $LameDll $Stage
# The LGPL notice next to the DLL. packaging/env.sh writes the same text for the other platforms.
Copy-Item (Join-Path $Root 'packaging\lame\COPYING') (Join-Path $Stage 'LICENSE-LAME.txt')
Set-Content -Path (Join-Path $Stage 'LAME.txt') -Value @"
CraftRadio encodes MP3 with LAME $LameVersion (https://lame.sourceforge.io), licensed under the
GNU LGPL (LICENSE-LAME.txt). LAME is not built into CraftRadio: it is the separate shared library
libmp3lame.dll next to craftradio.exe. To use your own build of LAME, build LAME $LameVersion or a
compatible version as a DLL from its source and replace that file.
"@

& (Join-Path $PSScriptRoot 'sign.ps1') @('craftradio.exe', 'craftradio-cli.exe', 'libmp3lame.dll' | ForEach-Object { Join-Path $Stage $_ })

# ---- MSI ---------------------------------------------------------------------------------------
# The setup wizard art comes from the app icon. Both images must be 24-bit BMPs.
# The banner is 493x58 at the top of the inner pages. The page title covers its left side.
# The dialog image is 493x312 for the Welcome and Finish pages. Their text covers the right side.
function New-InstallerArt([string] $IconPng, [string] $BannerOut, [string] $DialogOut) {
  Add-Type -AssemblyName System.Drawing
  $panelColour = [System.Drawing.ColorTranslator]::FromHtml('#f6efe6')
  $accent = [System.Drawing.ColorTranslator]::FromHtml('#b02c32')
  $icon = [System.Drawing.Image]::FromFile($IconPng)
  try {
    foreach ($spec in @(@($BannerOut, 493, 58), @($DialogOut, 493, 312))) {
      $bmp = New-Object System.Drawing.Bitmap $spec[1], $spec[2], ([System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
      $g = [System.Drawing.Graphics]::FromImage($bmp)
      try {
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $g.Clear([System.Drawing.Color]::White)
        if ($spec[2] -eq 58) {
          $g.DrawImage($icon, 493 - 46 - 8, 6, 46, 46)
        } else {
          $panel = New-Object System.Drawing.SolidBrush $panelColour
          $stripe = New-Object System.Drawing.SolidBrush $accent
          $g.FillRectangle($panel, 0, 0, 164, 312)
          $g.FillRectangle($stripe, 160, 0, 4, 312)
          $g.DrawImage($icon, 18, 64, 124, 124)
          $panel.Dispose(); $stripe.Dispose()
        }
      } finally { $g.Dispose() }
      $bmp.Save($spec[0], [System.Drawing.Imaging.ImageFormat]::Bmp)
      $bmp.Dispose()
    }
  } finally { $icon.Dispose() }
}
$Banner = Join-Path $Stage 'installer-banner.bmp'
$Dialog = Join-Path $Stage 'installer-dialog.bmp'
New-InstallerArt (Join-Path $Root "assets\app-icon\hicolor\256x256\apps\$AppId.png") $Banner $Dialog

$Msi = Join-Path $Dist "craftradio-$Version-windows-$Arch.msi"
& (Join-Path $PSScriptRoot 'check-icons.ps1')
Invoke-Native 'wix build' {
  wix build (Join-Path $PSScriptRoot 'craftradio.wxs') -arch $Arch `
    -ext WixToolset.UI.wixext -ext WixToolset.Util.wixext `
    -culture en-US -loc (Join-Path $PSScriptRoot 'craftradio.en-us.wxl') `
    -d "Version=$MsiVersion" -d "DisplayVersion=$Version" -d "BinDir=$Stage" -d "IconPath=$(Join-Path $Root 'assets\app-icon\craftradio.ico')" `
    -d "BannerBmp=$Banner" -d "DialogBmp=$Dialog" `
    -o $Msi
}
Invoke-Native 'MSI shortcut icon validation (ICE50)' {
  wix msi validate $Msi -ice ICE50 -intermediateFolder (Join-Path $Stage 'msi-validation')
}
# Keep the WiX debug symbols out of the release files.
Remove-Item -Force -ErrorAction SilentlyContinue ([IO.Path]::ChangeExtension($Msi, '.wixpdb'))
& (Join-Path $PSScriptRoot 'sign.ps1') $Msi

# ---- portable zip ------------------------------------------------------------------------------
$Portable = Join-Path $TargetDir "windows-package\craftradio-$Version-windows-$Arch-portable"
Remove-Item -Recurse -Force $Portable -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Portable | Out-Null
foreach ($f in 'craftradio.exe', 'craftradio-cli.exe', 'libmp3lame.dll', 'LICENSE-LAME.txt', 'LAME.txt') { Copy-Item (Join-Path $Stage $f) $Portable }
foreach ($f in 'README.md', 'LICENSE-MIT', 'LICENSE-APACHE') {
  $p = Join-Path $Root $f
  if (Test-Path $p) { Copy-Item $p $Portable }
}
$Zip = Join-Path $Dist "craftradio-$Version-windows-$Arch-portable.zip"
Remove-Item -Force $Zip -ErrorAction SilentlyContinue
Compress-Archive -Path $Portable -DestinationPath $Zip

# Run --version when this machine can run the build. The app must load libmp3lame.dll to start.
$HostArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
if ($Arch -ne 'arm64' -or $HostArch -eq 'arm64') {
  Invoke-Native 'craftradio.exe --version' { & (Join-Path $Stage 'craftradio.exe') --version | Out-Host }
  Invoke-Native 'craftradio-cli.exe --version' { & (Join-Path $Stage 'craftradio-cli.exe') --version | Out-Host }
} else {
  Write-Output "skipping --version: an $Arch build does not run on this $HostArch machine"
}
Get-Item $Msi, $Zip | Format-Table Name, Length
