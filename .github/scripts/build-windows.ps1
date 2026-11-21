[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet('i686-pc-windows-msvc', 'x86_64-pc-windows-msvc')]
    [string]$Target,
    [AllowEmptyString()]
    [string]$ReleaseTag = ''
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
} finally {
    Pop-Location
}
