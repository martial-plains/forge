# Build Forge for Windows (frontends/winui).
#
# Compiles forge.dll (the C ABI the WinUI app calls) and publishes the
# app. Needs: Rust (MSVC toolchain), the .NET 10 SDK and the Windows App SDK
# workload.
#
# UNVERIFIED: the WinUI app has not been compiled.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
cargo build --release -p forge-capi
Copy-Item "$root\target\release\forge.dll" "$root\frontends\winui\" -Force
Set-Location "$root\frontends\winui"
dotnet publish -c Release -r win-x64 --self-contained
Write-Host "done: frontends\winui\bin\Release\..."
