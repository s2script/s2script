# Native Windows x64/MSVC gate. Keep this as the single command run by ci-native.yml, mirroring
# ci-native.sh on Linux.
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
Set-Location (Resolve-Path (Join-Path $PSScriptRoot ".."))

Write-Host "== static Windows build contract =="
python scripts/check-windows-build.py

Write-Host "== cargo build =="
cargo build --locked --release

Write-Host "== cargo test -p s2script-core =="
cargo test --locked -p s2script-core

Write-Host "== CMake configure (Visual Studio 2022 x64) =="
cmake -S shim -B build/shim-windows -G "Visual Studio 17 2022" -A x64 `
    -DS2_CORE_LIB_DIR=release

Write-Host "== CMake build =="
cmake --build build/shim-windows --config Release --parallel

Write-Host "== CTest =="
ctest --test-dir build/shim-windows -C Release --output-on-failure

$coreDll = Resolve-Path "target/release/s2script_core.dll"
$coreImportCandidates = @(
    "target/release/s2script_core.dll.lib",
    "target/release/deps/s2script_core.dll.lib"
) | Where-Object { Test-Path $_ }
if ($coreImportCandidates.Count -lt 1) {
    throw "Rust core import library not found in target/release or target/release/deps"
}
$coreImport = Resolve-Path $coreImportCandidates[0]
$shimDll = Resolve-Path "build/shim-windows/Release/s2script.dll"
$stagedCore = Resolve-Path "build/shim-windows/Release/s2script_core.dll"
Write-Host "core DLL:       $coreDll"
Write-Host "core import:    $coreImport"
Write-Host "shim DLL:       $shimDll"
Write-Host "staged core:    $stagedCore"

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vsRoot = & $vswhere -latest -products * `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
$dumpbin = Get-ChildItem "$vsRoot\VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe" |
    Sort-Object FullName | Select-Object -Last 1 -ExpandProperty FullName
if (-not $dumpbin) {
    throw "dumpbin.exe not found in the selected Visual Studio installation"
}

Write-Host "== core export check =="
$coreExports = (& $dumpbin /nologo /exports $coreDll | Out-String)
@(
    "s2script_core_init_v2",
    "s2script_core_dispatch_hook",
    "s2script_core_dispatch_hook_post"
) | ForEach-Object {
    if ($coreExports -notmatch "(?m)\b$([regex]::Escape($_))\b") {
        throw "core DLL does not export $_"
    }
}

Write-Host "== shim export/import check =="
$shimExports = (& $dumpbin /nologo /exports $shimDll | Out-String)
if ($shimExports -notmatch "(?m)\bCreateInterface\b") {
    throw "s2script.dll does not export CreateInterface"
}
$shimImports = (& $dumpbin /nologo /imports $shimDll | Out-String)
if ($shimImports -notmatch "(?m)\bs2script_core_init_v2\b") {
    throw "s2script.dll does not import the required versioned core initializer"
}
@("s2script_core_dispatch_hook", "s2script_core_dispatch_hook_post") | ForEach-Object {
    if ($shimImports -match "(?m)\b$([regex]::Escape($_))\b") {
        throw "optional entry $_ became a hard PE import instead of explicit lookup"
    }
}

Write-Host "== SDK platform tests =="
npm ci --ignore-scripts
npm test --workspace packages/sdk

Write-Host "ci-native-windows: all native gates passed"
