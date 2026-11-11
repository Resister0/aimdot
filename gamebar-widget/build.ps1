<#
.SYNOPSIS
  Builds the Aimdot Game Bar widget into an MSIX and signs it (Windows, PowerShell).

.DESCRIPTION
  Needs Visual Studio 2022 (or newer) with the "Universal Windows Platform development" workload / UWP tools
  (C#, .NET Native) and a Windows 10/11 SDK. Nothing else - NuGet packages are restored by msbuild.

  Signed build (default): uses the Certum code-signing certificate from your certificate store (SimplySign Desktop
  must be connected, so Windows sees the certificate). The MSIX's Publisher has to be the certificate's subject
  exactly, so for the build only the script writes that subject into Package.appxmanifest and puts the placeholder
  ("CN=Resister0") back afterwards - the file in the repo never holds it.

  -NoSign: no certificate, no install - only builds the .msix (CI uses it to check the widget builds when the
  signing secrets aren't set; an unsigned MSIX can't be installed normally, so it is never released).

  -Dev: no certificate. Builds with the placeholder publisher and registers the unpacked package for this user
  (needs Windows Developer Mode: Settings > System > For developers). For trying the widget on your own PC.

.EXAMPLE
  cd ~\Desktop\Sonela\Aimdot; .\gamebar-widget\build.ps1 -Dev
.EXAMPLE
  cd ~\Desktop\Sonela\Aimdot; .\gamebar-widget\build.ps1 -Install
.EXAMPLE
  cd ~\Desktop\Sonela\Aimdot; .\gamebar-widget\build.ps1 -Thumbprint 0123ABCD...
#>
[CmdletBinding()]
param(
    # Build without signing and register it for this user (Developer Mode).
    [switch]$Dev,
    # Build only: no certificate, no signing, no install (CI without signing secrets).
    [switch]$NoSign,
    # Signed build: install it for this user afterwards.
    [switch]$Install,
    # Signed build: which certificate (SHA1 thumbprint). Default: the only valid code-signing certificate with a
    # private key in CurrentUser\My.
    [string]$Thumbprint,
    # x64 (default) or ARM64.
    [ValidateSet('x64', 'ARM64')]
    [string]$Platform = 'x64',
    # Timestamp server (Certum's; must be RFC 3161).
    [string]$TimestampUrl = 'http://time.certum.pl'
)

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$manifest = Join-Path $here 'Package.appxmanifest'
$project = Join-Path $here 'CrosshairWidget.csproj'
$out = Join-Path $here 'out'
$pkgDir = Join-Path $out 'packages'
$placeholder = 'CN=Resister0'

function Step($text) { Write-Host "==> $text" -ForegroundColor Cyan }

# ---- Tools -------------------------------------------------------------------------------------------------------

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $vswhere)) { throw 'Visual Studio not found (no vswhere.exe). Install Visual Studio with the UWP workload.' }
$msbuild = & $vswhere -latest -prerelease -products * -requires Microsoft.Component.MSBuild -find 'MSBuild\**\Bin\MSBuild.exe' | Select-Object -First 1
if (-not $msbuild) { throw 'MSBuild not found. Install Visual Studio with the "Universal Windows Platform development" workload.' }

$kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10'
$sdk = Get-ChildItem (Join-Path $kits 'Platforms\UAP') -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match '^10\.0\.\d+\.\d+$' } | Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
if (-not $sdk) { throw "No Windows 10/11 SDK in $kits\Platforms\UAP. Add one in the Visual Studio Installer." }
$signtool = Get-ChildItem (Join-Path $kits 'bin') -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match '\\x64\\signtool\.exe$' } | Sort-Object FullName -Descending | Select-Object -First 1
Write-Host "MSBuild:     $msbuild"
Write-Host "Windows SDK: $($sdk.Name)"

# ---- Certificate (signed build) ------------------------------------------------------------------------------------

$cert = $null
if (-not $Dev -and -not $NoSign) {
    if (-not $signtool) { throw 'signtool.exe not found (Windows SDK). Add the Windows SDK in the Visual Studio Installer.' }
    $codeSigning = '1.3.6.1.5.5.7.3.3'
    $certs = Get-ChildItem Cert:\CurrentUser\My | Where-Object {
        $_.HasPrivateKey -and $_.NotAfter -gt (Get-Date) -and
        ($_.EnhancedKeyUsageList | Where-Object { $_.ObjectId -eq $codeSigning })
    }
    if ($Thumbprint) {
        $cert = $certs | Where-Object { $_.Thumbprint -eq $Thumbprint.Replace(' ', '').ToUpper() } | Select-Object -First 1
        if (-not $cert) { throw "No valid code-signing certificate with thumbprint $Thumbprint in CurrentUser\My. Is SimplySign Desktop connected?" }
    } elseif (@($certs).Count -eq 1) {
        $cert = @($certs)[0]
    } elseif (@($certs).Count -eq 0) {
        throw 'No code-signing certificate found. Connect SimplySign Desktop (Certum) and run again - or use -Dev to try it unsigned.'
    } else {
        $certs | ForEach-Object { Write-Host "  $($_.Thumbprint)  valid until $($_.NotAfter.ToString('yyyy-MM-dd'))" }
        throw 'More than one code-signing certificate: pick one with -Thumbprint.'
    }
    # Not printed: the subject holds the certificate owner's name. Only the thumbprint is shown.
    Write-Host "Certificate: $($cert.Thumbprint) (valid until $($cert.NotAfter.ToString('yyyy-MM-dd')))"
}

# ---- Build ---------------------------------------------------------------------------------------------------------

$backup = "$manifest.orig"
Copy-Item $manifest $backup -Force
try {
    if ($cert) {
        # Publisher = the certificate's subject, exactly (otherwise signing fails). Set through the XML DOM so quotes
        # and commas in the subject are escaped correctly.
        $xml = New-Object System.Xml.XmlDocument
        $xml.PreserveWhitespace = $true
        $xml.Load($manifest)
        $ns = New-Object System.Xml.XmlNamespaceManager($xml.NameTable)
        $ns.AddNamespace('m', 'http://schemas.microsoft.com/appx/manifest/foundation/windows10')
        $xml.SelectSingleNode('/m:Package/m:Identity', $ns).SetAttribute('Publisher', $cert.Subject)
        $xml.Save($manifest)
    }

    if (Test-Path $pkgDir) { Remove-Item $pkgDir -Recurse -Force }
    Step "Building (Release, $Platform) - the first build downloads NuGet packages and takes a few minutes"
    & $msbuild $project /restore /m /nologo /v:minimal `
        /p:Configuration=Release /p:Platform=$Platform `
        /p:TargetPlatformVersion=$($sdk.Name) `
        /p:UapAppxPackageBuildMode=SideloadOnly /p:AppxBundle=Never `
        /p:AppxPackageSigningEnabled=false /p:GenerateAppInstallerFile=false `
        "/p:AppxPackageDir=$pkgDir/"   # must end in a slash; "/" because "\" + closing quote breaks argument parsing
    if ($LASTEXITCODE -ne 0) { throw "msbuild failed (exit code $LASTEXITCODE)" }
} finally {
    # Always put the placeholder manifest back (byte for byte).
    Move-Item $backup $manifest -Force
}

$built = Get-ChildItem $pkgDir -Recurse -Filter *.msix | Where-Object { $_.FullName -notmatch '\\Dependencies\\' } |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $built) { throw "No .msix found in $pkgDir" }
# Framework packages the widget needs (.NET Native runtime, VC libs). Windows usually has them already; they are
# passed to Add-AppxPackage in case it doesn't.
$deps = @(Get-ChildItem $pkgDir -Recurse -Include *.appx, *.msix -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match "\\Dependencies\\$Platform\\" } | ForEach-Object { $_.FullName })

$version = ([xml](Get-Content $manifest)).Package.Identity.Version
$final = Join-Path $out ("AimdotWidget-$version-$Platform" + $(if ($Dev) { '-dev' } elseif ($NoSign) { '-unsigned' } else { '' }) + '.msix')
Copy-Item $built.FullName $final -Force

if ($NoSign) {
    Write-Host ''
    Write-Host "Built (not signed, not installed): $final" -ForegroundColor Green
    return
}

# Remove any installed copy first (a dev copy and a signed copy have different publishers and would both show up
# in Game Bar).
function Remove-Installed {
    Get-AppxPackage -Name 'Resister0.AimdotWidget' | ForEach-Object {
        Step "Removing the installed widget $($_.PackageFullName)"
        Remove-AppxPackage $_.PackageFullName
    }
}

if ($Dev) {
    $layout = Join-Path $out 'dev-layout'
    if (Test-Path $layout) { Remove-Item $layout -Recurse -Force }
    $zip = Join-Path $out 'dev.zip'
    Copy-Item $final $zip -Force
    Expand-Archive $zip $layout -Force
    Remove-Item $zip
    Remove-Installed
    Step 'Registering the unsigned widget for this user (needs Developer Mode)'
    if ($deps.Count) { Add-AppxPackage -Register (Join-Path $layout 'AppxManifest.xml') -DependencyPath $deps }
    else { Add-AppxPackage -Register (Join-Path $layout 'AppxManifest.xml') }
    Write-Host ''
    Write-Host 'Done. Press Win+G, open the Widget menu and pick "Aimdot".' -ForegroundColor Green
    return
}

Step 'Signing'
& $signtool.FullName sign /fd SHA256 /sha1 $cert.Thumbprint /tr $TimestampUrl /td SHA256 $final
if ($LASTEXITCODE -ne 0) { throw "signtool sign failed (exit code $LASTEXITCODE)" }
& $signtool.FullName verify /pa $final
if ($LASTEXITCODE -ne 0) { throw "signtool verify failed (exit code $LASTEXITCODE)" }

if ($Install) {
    Remove-Installed
    Step 'Installing'
    if ($deps.Count) { Add-AppxPackage -Path $final -DependencyPath $deps } else { Add-AppxPackage -Path $final }
    Write-Host 'Installed. Press Win+G, open the Widget menu and pick "Aimdot".' -ForegroundColor Green
}

Write-Host ''
Write-Host "Signed widget: $final" -ForegroundColor Green
