param(
  [switch]$Build,
  [string]$Arch = "x64",
  [string]$Version = "",
  [string]$OutputRoot = ""
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Split-Path -Parent $ScriptDir
$FrontendDir = Join-Path $RepoRoot "frontend"
$TauriDir = Join-Path $RepoRoot "src-tauri"
$ReleaseDir = Join-Path $TauriDir "target\release"

if (-not $OutputRoot) {
  $OutputRoot = Join-Path $RepoRoot "dist\portable\windows-$Arch"
}

function Get-CargoVersion {
  $cargoToml = Join-Path $TauriDir "Cargo.toml"
  $versionLine = Get-Content $cargoToml | Where-Object { $_ -match '^\s*version\s*=' } | Select-Object -First 1
  if ($versionLine -match '"([^"]+)"') {
    return $Matches[1]
  }
  throw "Unable to read package version from $cargoToml"
}

function Find-WebView2Loader {
  $direct = Join-Path $ReleaseDir "WebView2Loader.dll"
  if (Test-Path $direct) {
    return $direct
  }

  $targetDir = Join-Path $TauriDir "target"
  $all = @(Get-ChildItem -Path $targetDir -Filter "WebView2Loader.dll" -Recurse -ErrorAction SilentlyContinue)
  if ($all.Count -eq 0) {
    throw "WebView2Loader.dll was not found under $targetDir. Build the Windows target first."
  }

  $preferred = $all |
    Where-Object { $_.FullName -match "\\$Arch\\WebView2Loader\.dll$" } |
    Select-Object -First 1

  if ($preferred) {
    return $preferred.FullName
  }

  return ($all | Select-Object -First 1).FullName
}

if ($Build) {
  Push-Location $FrontendDir
  npm run build
  Pop-Location

  Push-Location $TauriDir
  cargo build --release
  Pop-Location
}

if (-not $Version) {
  $Version = Get-CargoVersion
}

$ExePath = Join-Path $ReleaseDir "echo.exe"
if (-not (Test-Path $ExePath)) {
  throw "Missing $ExePath. Run this script with -Build, or build the Windows release first."
}

$LoaderPath = Find-WebView2Loader
$PackageName = "Echo-$Version-windows-$Arch-portable"
$StageDir = Join-Path $OutputRoot $PackageName
$ZipPath = Join-Path $OutputRoot "$PackageName.zip"

if (Test-Path $StageDir) {
  Remove-Item $StageDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

Copy-Item $ExePath (Join-Path $StageDir "echo.exe") -Force
Copy-Item $LoaderPath (Join-Path $StageDir "WebView2Loader.dll") -Force

@{
  version = $Version
  executable = "echo.exe"
} | ConvertTo-Json | Set-Content -Path (Join-Path $StageDir "portable.json") -Encoding UTF8

if (Test-Path $ZipPath) {
  Remove-Item $ZipPath -Force
}
Compress-Archive -Path (Join-Path $StageDir "*") -DestinationPath $ZipPath -Force

$ZipHash = (Get-FileHash $ZipPath -Algorithm SHA256).Hash.ToLowerInvariant()
$ZipSize = (Get-Item $ZipPath).Length

Write-Host "Portable package created:"
Write-Host "  $ZipPath"
Write-Host ""
Write-Host "Package metadata:"
Write-Host "  sha256: $ZipHash"
Write-Host "  size:   $ZipSize"
Write-Host ""
Write-Host "Included WebView2 loader:"
Write-Host "  $LoaderPath"
