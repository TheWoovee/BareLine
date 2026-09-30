# SPDX-License-Identifier: MPL-2.0
# Dot-source only. Shared by build-preview.ps1, build-configured.ps1 and the
# supply-chain clean builds so that release builds and the reproducibility
# replicas use the same path normalization and record the same tool identity.

# Remaps every build-machine path that reaches panic locations and debug info:
# the checkout, the Cargo target directory, Cargo's registry/git sources under
# CARGO_HOME and the rustup toolchain sysroot.
function Get-ReleaseRemapFlags([string]$SourceRoot, [string]$TargetDir) {
    $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path ([Environment]::GetFolderPath('UserProfile')) '.cargo' }
    # rust-toolchain.toml selects the toolchain, so ask from the source root.
    Push-Location $SourceRoot
    try { $sysroot = (& rustc --print sysroot) -join '' } finally { Pop-Location }
    if ($LASTEXITCODE -ne 0 -or -not $sysroot.Trim()) { throw 'rustc did not report its sysroot.' }
    $prefixes = @(
        [pscustomobject]@{ Path = $cargoHome; Mapped = '/bareline/cargo-home' },
        [pscustomobject]@{ Path = $sysroot.Trim(); Mapped = '/bareline/rust-sysroot' },
        [pscustomobject]@{ Path = $SourceRoot; Mapped = '/bareline/source' },
        [pscustomobject]@{ Path = $TargetDir; Mapped = '/bareline/target' }
    )
    # rustc applies the last matching mapping, so a nested directory (the
    # default target/ inside the checkout) must follow its parent.
    return @($prefixes | ForEach-Object { $_.Path = [IO.Path]::GetFullPath($_.Path).TrimEnd('\', '/'); $_ } |
        Sort-Object { $_.Path.Length } | ForEach-Object { "--remap-path-prefix=$($_.Path)=$($_.Mapped)" })
}

# Versions of the native tools that shape the executables and installer. The
# MSVC and SDK selection mirrors rustc/cc: a developer prompt's environment
# first, otherwise the newest Visual Studio C++ tools and Windows 10/11 SDK.
function Get-BuildToolIdentity([string]$SourceRoot, [string]$Iscc) {
    Push-Location $SourceRoot
    try {
        $rustc = (& rustc -vV) -join "`n"
        if ($LASTEXITCODE -ne 0) { throw 'rustc version unavailable.' }
        $cargo = (& cargo -V) -join ''
        if ($LASTEXITCODE -ne 0) { throw 'cargo version unavailable.' }
    } finally { Pop-Location }
    $toolsDirectory = $env:VCToolsInstallDir
    if (-not $toolsDirectory) {
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
        if (Test-Path -LiteralPath $vswhere -PathType Leaf) {
            $installation = @(& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath) | Select-Object -First 1
            $default = if ($installation) { Join-Path $installation 'VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt' }
            if ($default -and (Test-Path -LiteralPath $default -PathType Leaf)) {
                $toolsDirectory = Join-Path $installation ('VC/Tools/MSVC/' + (Get-Content -LiteralPath $default -TotalCount 1).Trim())
            }
        }
    }
    $linker = if ($toolsDirectory) { Join-Path $toolsDirectory 'bin/Hostx64/x64/link.exe' }
    $linkVersion = if ($linker -and (Test-Path -LiteralPath $linker -PathType Leaf)) { (Get-Item -LiteralPath $linker).VersionInfo.FileVersion } else { 'unavailable' }
    $sdkVersion = if ($env:WindowsSDKVersion) { $env:WindowsSDKVersion.TrimEnd('\') } else { 'unavailable' }
    if (-not $env:WindowsSDKVersion) {
        $kits = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots' -Name KitsRoot10 -ErrorAction SilentlyContinue
        if ($kits -and (Test-Path -LiteralPath (Join-Path $kits.KitsRoot10 'Lib'))) {
            $newest = Get-ChildItem -LiteralPath (Join-Path $kits.KitsRoot10 'Lib') -Directory |
                Where-Object { $_.Name -match '^\d+(\.\d+){3}$' -and (Test-Path -LiteralPath (Join-Path $_.FullName 'um/x64/kernel32.lib')) } |
                Sort-Object { [version]$_.Name } | Select-Object -Last 1
            if ($newest) { $sdkVersion = $newest.Name }
        }
    }
    $inno = if ($Iscc) { (Get-Item -LiteralPath $Iscc).VersionInfo.ProductVersion } else { 'not used' }
    return [ordered]@{
        schema_version = 1
        rustc = $rustc
        cargo = $cargo
        msvc_tools = if ($toolsDirectory) { Split-Path -Leaf $toolsDirectory.TrimEnd('\', '/') } else { 'unavailable' }
        link_exe = $linkVersion
        windows_sdk = $sdkVersion
        inno_setup = $inno
        runner_image = if ($env:ImageOS) { "$($env:ImageOS) $($env:ImageVersion)" } else { 'not a hosted runner' }
    }
}
