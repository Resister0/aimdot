<#
.SYNOPSIS
  Tests the Game Bar widget without the Aimdot app: puts a sample crosshair into the widget's data folder,
  exactly the way the app does it (overlay/src/gamebar.rs made the sample pictures in demo\).

.DESCRIPTION
  Run it while the widget is open in Game Bar: the widget should change within a moment, without reopening it
  (that checks the folder watching). The widget must have been opened once (Win+G > Widget menu > Aimdot),
  so its data folder exists.

.EXAMPLE
  cd ~\Desktop\Sonela\Aimdot; .\gamebar-widget\demo.ps1
.EXAMPLE
  cd ~\Desktop\Sonela\Aimdot; .\gamebar-widget\demo.ps1 -Rainbow -OffsetX 40
.EXAMPLE
  cd ~\Desktop\Sonela\Aimdot; .\gamebar-widget\demo.ps1 -Hide
#>
[CmdletBinding()]
param(
    [switch]$Rainbow,
    [switch]$Hide,
    [switch]$Recenter,
    [int]$OffsetX = 0,
    [int]$OffsetY = 0,
    # 0 cycle, 1 wave, 2 spin
    [ValidateRange(0, 2)]
    [int]$RainbowMode = 2
)
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$demo = Join-Path $here 'demo'

$dirs = @(Get-ChildItem (Join-Path $env:LOCALAPPDATA 'Packages') -Directory -Filter 'Resister0.AimdotWidget_*' |
    ForEach-Object { Join-Path $_.FullName 'LocalState' } | Where-Object { Test-Path $_ })
if (-not $dirs.Count) { throw 'Widget folder not found. Install the widget and open it once in Game Bar (Win+G).' }

$seq = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$recenterValue = if ($Recenter) { $seq } else { 0 }
$ini = @"
# Written by demo.ps1 (same format as the Aimdot app)
format=1
seq=$seq
visible=$(if ($Hide) { 0 } else { 1 })
width=50
height=50
offset_x=$OffsetX
offset_y=$OffsetY
monitor=0
recenter=$recenterValue
rainbow=$(if ($Rainbow) { 1 } else { 0 })
rainbow_mode=$RainbowMode
rainbow_speed=1
rainbow_sat=1
"@

foreach ($d in $dirs) {
    # Same order as the app: pictures first (temp name, then rename), the ini last.
    foreach ($name in 'crosshair.png', 'crosshair-base.png', 'crosshair-fill.png') {
        Copy-Item (Join-Path $demo $name) (Join-Path $d "$name.tmp") -Force
        Move-Item (Join-Path $d "$name.tmp") (Join-Path $d $name) -Force
    }
    [IO.File]::WriteAllText((Join-Path $d 'crosshair.ini.tmp'), $ini.Replace("`r`n", "`n"))
    Move-Item (Join-Path $d 'crosshair.ini.tmp') (Join-Path $d 'crosshair.ini') -Force
    Write-Host "Written: $d"
}
