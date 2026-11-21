[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet('i686-pc-windows-msvc', 'x86_64-pc-windows-msvc')]
    [string]$Target,
    [AllowEmptyString()]
    [string]$ReleaseTag = '',
    # Build only the ZIP (no WiX Toolset needed).
    [switch]$SkipMsi
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Push-Location (Join-Path $PSScriptRoot '../..')
try {
    $metadataJson = cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Failed to read Cargo metadata.' }
    $metadata = $metadataJson | ConvertFrom-Json
    $package = $metadata.packages | Where-Object name -eq 'quokka-file-manager'
    $version = $package.version
    if ($ReleaseTag -and $ReleaseTag -cne "v$version") {
        throw "Release tag '$ReleaseTag' must match Cargo.toml version 'v$version'."
    }

    # Windows integration tests share shell state; avoid concurrent shell calls.
    cargo test --workspace --locked --target $Target -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'Tests failed; release build cancelled.' }

    cargo build --release --locked --target $Target --bin quokka
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }

    $arch = if ($Target -eq 'i686-pc-windows-msvc') { 'x86' } else { 'x64' }
    $packageDir = "release/windows-$arch"
    New-Item -ItemType Directory -Path $packageDir -Force | Out-Null
    $exe = Join-Path $metadata.target_directory "$Target/release/quokka.exe"

    # Check the PE machine field so an incorrectly targeted binary cannot ship.
    $stream = [IO.File]::OpenRead($exe)
    $reader = [IO.BinaryReader]::new($stream)
    try {
        $stream.Position = 0x3c
        $peOffset = $reader.ReadInt32()
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) { throw 'Invalid PE signature.' }
        $machine = $reader.ReadUInt16()
        $expected = if ($arch -eq 'x86') { 0x014c } else { 0x8664 }
        if ($machine -ne $expected) { throw "Unexpected executable architecture: $machine." }
    } finally {
        $reader.Dispose()
    }

    Copy-Item -LiteralPath $exe -Destination "$packageDir/quokka.exe" -Force
    Copy-Item -LiteralPath README.md, CHANGELOG.md -Destination $packageDir -Force
    $archive = "release/quokka-v$version-windows-$arch.zip"
    Compress-Archive -Path "$packageDir/*" -DestinationPath $archive -Force
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $([IO.Path]::GetFileName($archive))" |
        Set-Content -LiteralPath "$archive.sha256" -Encoding ascii
    Write-Host "Package ready: $archive"

    if (-not $SkipMsi) {
        # MSI installer (WiX Toolset v5: `dotnet tool install --global wix --version 5.0.2`
        # and `wix extension add --global WixToolset.UI.wixext/5.0.2`).
        if (-not (Get-Command wix -ErrorAction SilentlyContinue)) {
            throw 'WiX Toolset (wix) is not installed; install it or pass -SkipMsi.'
        }
        # MSI ProductVersion is numeric only: 0.2.0-rc.1 -> 0.2.0
        $msiVersion = ($version -split '[-+]')[0]
        $wixDir = Join-Path $metadata.target_directory "wix/$arch"
        New-Item -ItemType Directory -Path $wixDir -Force | Out-Null

        # License page of the installer: LICENSE as plain RTF
        $licenseText = Get-Content -LiteralPath LICENSE -Raw -Encoding utf8
        $escaped = $licenseText.Replace('\', '\\').Replace('{', '\{').Replace('}', '\}')
        $escaped = $escaped -replace "`r?`n", "\par`r`n"
        $rtf = '{\rtf1\ansi\deff0{\fonttbl{\f0 Segoe UI;}}\fs18' + "`r`n" + $escaped + "`r`n}"
        $licenseRtf = Join-Path $wixDir 'License.rtf'
        Set-Content -LiteralPath $licenseRtf -Value $rtf -Encoding ascii

        $msi = "release/quokka-v$version-windows-$arch.msi"
        wix build installer/quokka.wxs -arch $arch -ext WixToolset.UI.wixext `
            -d "Version=$msiVersion" -d "ExePath=$packageDir/quokka.exe" `
            -d "ReadmePath=README.md" -d "ChangelogPath=CHANGELOG.md" -d "LicensePath=LICENSE" `
            -d "LicenseRtf=$licenseRtf" -intermediatefolder $wixDir -o $msi
        if ($LASTEXITCODE -ne 0) { throw 'MSI build failed.' }
        Remove-Item -LiteralPath ([IO.Path]::ChangeExtension($msi, '.wixpdb')) -ErrorAction SilentlyContinue
        $hash = (Get-FileHash -LiteralPath $msi -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $([IO.Path]::GetFileName($msi))" |
            Set-Content -LiteralPath "$msi.sha256" -Encoding ascii
        Write-Host "Installer ready: $msi"
    }
} finally {
    Pop-Location
}
