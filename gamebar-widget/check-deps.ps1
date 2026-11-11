<#
.SYNOPSIS
  Checks that the framework packages shipped with the Game Bar widget (out\deps\*.appx) cover what the widget's
  MSIX asks for (its <PackageDependency Name MinVersion>), so Add-AppxPackage -DependencyPath can install it on a PC
  that doesn't have them (Windows LTSC: no Store, no App Installer - 11 Nov 2026: "depends on a framework that could
  not be found ... Microsoft.NET.Native.Framework.2.3 ... minimum version 2.3.60521.0").

.DESCRIPTION
  Reads AppxManifest.xml out of each package (they are zip files). A dependency counts as covered when a package in
  -Deps has the same Name, a Version >= MinVersion and the widget's architecture (or neutral).
  Every package in -Deps must also carry a valid Microsoft signature, and the .NET Native ones that come from NuGet
  must match their pinned SHA256 (below).
  Prints one line per package / dependency; exit code 0 = all fine, 1 = something missing or not trusted (build.ps1
  then stops: an installer must never carry a widget it can't install).

.EXAMPLE
  cd ~\Desktop\Sonela\Aimdot; .\gamebar-widget\check-deps.ps1 -Package gamebar-widget\out\AimdotWidget-1.0.0.0-x64.msix -Deps (Get-ChildItem gamebar-widget\out\deps\*.appx).FullName
#>
param(
    [Parameter(Mandatory)] [string]$Package,
    [string[]]$Deps = @()
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem

# SHA256 of the framework packages the widget build takes from NuGet (UWP 6.2.14: compiler 2.2.12-rel-31116-00 +
# runtime.win10-x64.Microsoft.Net.Native.SharedLibrary 2.2.8-rel-31116-00), checked 12 Nov 2026. A package with this
# name + version but another hash is refused. Other packages (VC libs from the Windows SDK) only need the signature.
$pinned = @{
    'Microsoft.NET.Native.Framework.2.2|2.2.29512.0|x64' = 'C195047F4AF9C3FF3D92C40E55A901CF7CFA08A679B8CA2543BB104BAB2F7FF1'
    'Microsoft.NET.Native.Runtime.2.2|2.2.28604.0|x64'   = '27FAE660D8C609E020703BB463B6601E62811DC75DB19975BCB7D68D959B527D'
}

function Read-Manifest([string]$path) {
    $zip = [IO.Compression.ZipFile]::OpenRead((Resolve-Path $path).Path)
    try {
        $entry = $zip.GetEntry('AppxManifest.xml')
        if (-not $entry) { throw "no AppxManifest.xml in $path" }
        $reader = New-Object IO.StreamReader($entry.Open())
        try { [xml]$reader.ReadToEnd() } finally { $reader.Dispose() }
    } finally { $zip.Dispose() }
}

$bad = 0
$pkg = Read-Manifest $Package
$arch = $pkg.Package.Identity.ProcessorArchitecture
$have = foreach ($d in $Deps) {
    $id = (Read-Manifest $d).Package.Identity
    $file = Split-Path -Leaf $d
    $sig = Get-AuthenticodeSignature $d
    $signer = if ($sig.SignerCertificate) { ($sig.SignerCertificate.Subject -split ',')[0] } else { 'none' }
    $hash = (Get-FileHash $d -Algorithm SHA256).Hash
    $key = "$($id.Name)|$($id.Version)|$($id.ProcessorArchitecture)"
    $trusted = $sig.Status -eq 'Valid' -and $signer -eq 'CN=Microsoft Corporation'
    $pin = if ($pinned.ContainsKey($key)) { if ($pinned[$key] -eq $hash) { 'pinned ok' } else { $trusted = $false; 'PINNED HASH DIFFERS' } } else { 'not pinned' }
    Write-Host "  package  $file : $($id.Name) $($id.Version) $($id.ProcessorArchitecture), signature $($sig.Status) $signer, $pin, sha256 $hash"
    if (-not $trusted) { $bad++; Write-Host "  REFUSED  $file (needs a valid Microsoft signature and, if pinned, the pinned hash)"; continue }
    [pscustomobject]@{ Name = $id.Name; Version = [version]$id.Version; Arch = $id.ProcessorArchitecture; File = $file }
}
foreach ($need in @($pkg.Package.Dependencies.PackageDependency)) {
    $min = [version]$need.MinVersion
    $hit = $have | Where-Object { $_.Name -eq $need.Name -and $_.Version -ge $min -and ($_.Arch -eq $arch -or $_.Arch -eq 'neutral') } |
        Sort-Object Version -Descending | Select-Object -First 1
    if ($hit) {
        Write-Host "  ok       $($need.Name) >= $min : $($hit.File) ($($hit.Version))"
    } else {
        $bad++
        $near = ($have | Where-Object Name -eq $need.Name | ForEach-Object { "$($_.File) $($_.Version) $($_.Arch)" }) -join ', '
        Write-Host "  MISSING  $($need.Name) >= $min ($arch)$(if ($near) { " - only: $near" })"
    }
}
if ($bad) { exit 1 }
exit 0
