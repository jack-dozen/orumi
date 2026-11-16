# Build the vscroll-wm release binary and stage it as a Tauri externalBin sidecar.
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $root "vscroll-wm\Cargo.toml"
$src = Join-Path $root "vscroll-wm\target\release\vscroll-wm.exe"
$dstDir = Join-Path $root "src-tauri\binaries"
$dst = Join-Path $dstDir "vscroll-wm-x86_64-pc-windows-msvc.exe"

cargo build --release --manifest-path $manifest
New-Item -ItemType Directory -Force -Path $dstDir | Out-Null

$needsCopy = -not (Test-Path $dst) -or
  ((Get-Item $src).Length -ne (Get-Item $dst).Length) -or
  ((Get-Item $src).LastWriteTimeUtc -gt (Get-Item $dst).LastWriteTimeUtc)

if ($needsCopy) {
  Copy-Item -Path $src -Destination $dst -Force
}
