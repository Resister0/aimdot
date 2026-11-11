<#
.SYNOPSIS
  Checks that the framework packages shipped with the Game Bar widget (out\deps\*.appx) cover what the widget's
  MSIX asks for (its <PackageDependency Name MinVersion>), so Add-AppxPackage -DependencyPath can install it on a PC
  that doesn't have them (Windows LTSC: no Store, no App Installer - 11 Nov 2026: "depends on a framework that could
  not be found ... Microsoft.NET.Native.Framework.2.3 ... minimum version 2.3.60521.0").

.DESCRIPTION
  Reads AppxManifest.xml out of each package (they are zip files). A dependency counts as covered when a package in
  -Deps has the same Name, a Version >= MinVersion and the widget's architecture (or neutral). Prints one line per
  dependency; exit code 0 = all covered, 1 = something missing (the widget then only installs where Windows has it).

.EXAMPLE
  cd ~\Desktop\Sonela\Aimdot; .\gamebar-widget\check-deps.ps1 -Package gamebar-widget\out\AimdotWidget-1.0.0.0-x64.msix -Deps (Get-ChildItem gamebar-widget\out\deps\*.appx).FullName
#>
param(
    [Parameter(Mandatory)] [string]$Package,
    [string[]]$Deps = @()
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Read-Manifest([string]$path) {
    $zip = [IO.Compression.ZipFile]::OpenRead((Resolve-Path $path).Path)
    try {
        $entry = $zip.GetEntry('AppxManifest.xml')
        if (-not $entry) { throw "no AppxManifest.xml in $path" }
        $reader = New-Object IO.StreamReader($entry.Open())
        try { [xml]$reader.ReadToEnd() } finally { $reader.Dispose() }
    } finally { $zip.Dispose() }
}

$pkg = Read-Manifest $Package
$arch = $pkg.Package.Identity.ProcessorArchitecture
$have = foreach ($d in $Deps) {
    $id = (Read-Manifest $d).Package.Identity
    [pscustomobject]@{ Name = $id.Name; Version = [version]$id.Version; Arch = $id.ProcessorArchitecture; File = Split-Path -Leaf $d }
}
$missing = 0
foreach ($need in @($pkg.Package.Dependencies.PackageDependency)) {
    $min = [version]$need.MinVersion
    $hit = $have | Where-Object { $_.Name -eq $need.Name -and $_.Version -ge $min -and ($_.Arch -eq $arch -or $_.Arch -eq 'neutral') } |
        Sort-Object Version -Descending | Select-Object -First 1
    if ($hit) {
        Write-Host "  ok       $($need.Name) >= $min : $($hit.File) ($($hit.Version))"
    } else {
        $missing++
        $near = ($have | Where-Object Name -eq $need.Name | ForEach-Object { "$($_.File) $($_.Version) $($_.Arch)" }) -join ', '
        Write-Host "  MISSING  $($need.Name) >= $min ($arch)$(if ($near) { " - only: $near" })"
    }
}
if ($missing) { exit 1 }
exit 0
