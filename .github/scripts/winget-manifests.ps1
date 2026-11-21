# Writes winget manifests (version, installer, defaultLocale; schema 1.6.0) for the
# MSI packages of a release, in the folder layout of microsoft/winget-pkgs:
#   <OutDir>/manifests/n/Natterum/QuokkaFileManager/<version>/
# Hashes come from the .msi.sha256 files next to the packages; the ProductCode is read
# from each MSI so winget can match the installed program.
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$Version,
    [Parameter(Mandatory)] [string]$Repository,
    [string]$ReleaseDir = 'release',
    [string]$OutDir = 'release/winget'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$id = 'Natterum.QuokkaFileManager'
$schema = '1.6.0'
$tag = "v$Version"
$base = "https://github.com/$Repository/releases/download/$tag"

function Get-MsiProperty([string]$Path, [string]$Name) {
    try {
        $installer = New-Object -ComObject WindowsInstaller.Installer
        $db = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @((Resolve-Path $Path).Path, 0))
        $view = $db.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $db, @("SELECT Value FROM Property WHERE Property='$Name'"))
        $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null) | Out-Null
        $record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
        if ($record) { return $record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, 1) }
    } catch {
        Write-Warning "Could not read $Name from ${Path}: $_"
    }
    return $null
}

$installers = foreach ($arch in 'x64', 'x86') {
    $name = "quokka-v$Version-windows-$arch.msi"
    $msi = Join-Path $ReleaseDir $name
    $sumFile = "$msi.sha256"
    if (-not (Test-Path -LiteralPath $sumFile)) { throw "Missing $sumFile" }
    $sha = ((Get-Content -LiteralPath $sumFile -Raw).Trim() -split '\s+')[0].ToUpperInvariant()
    if ($sha -notmatch '^[0-9A-F]{64}$') { throw "Bad checksum in $sumFile" }
    $entry = @"
  - Architecture: $arch
    InstallerUrl: $base/$name
    InstallerSha256: $sha
"@
    if (Test-Path -LiteralPath $msi) {
        $code = Get-MsiProperty $msi 'ProductCode'
        if ($code) { $entry += "`n    ProductCode: '$code'" }
    }
    $entry
}

$dir = Join-Path $OutDir "manifests/n/Natterum/QuokkaFileManager/$Version"
# absolute: .NET resolves relative paths against the process folder, not the PowerShell location
$dir = (New-Item -ItemType Directory -Path $dir -Force).FullName
$header = "# yaml-language-server: `$schema=https://aka.ms/winget-manifest"

$versionYaml = @"
$header.version.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $Version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: $schema
"@

$installerYaml = @"
$header.installer.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $Version
InstallerType: wix
Scope: machine
InstallModes:
  - interactive
  - silent
  - silentWithProgress
UpgradeBehavior: install
Installers:
$($installers -join "`n")
ManifestType: installer
ManifestVersion: $schema
"@

$localeYaml = @"
$header.defaultLocale.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $Version
PackageLocale: en-US
Publisher: Natterum
PublisherUrl: https://github.com/Natterum
PublisherSupportUrl: https://github.com/$Repository/issues
PackageName: Quokka File Manager
PackageUrl: https://github.com/$Repository
License: MIT
LicenseUrl: https://github.com/$Repository/blob/main/LICENSE
ShortDescription: A fast two-pane file manager with tabs, F1-F12 keys and background operations.
Moniker: quokka
Tags:
  - file-manager
  - two-pane
  - explorer
  - sftp
  - ftp
ReleaseNotesUrl: https://github.com/$Repository/releases/tag/$tag
ManifestType: defaultLocale
ManifestVersion: $schema
"@

$utf8 = [Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText((Join-Path $dir "$id.yaml"), $versionYaml.Replace("`r`n", "`n") + "`n", $utf8)
[IO.File]::WriteAllText((Join-Path $dir "$id.installer.yaml"), $installerYaml.Replace("`r`n", "`n") + "`n", $utf8)
[IO.File]::WriteAllText((Join-Path $dir "$id.locale.en-US.yaml"), $localeYaml.Replace("`r`n", "`n") + "`n", $utf8)
Write-Host "winget manifests: $dir"
