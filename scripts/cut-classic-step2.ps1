# Step 2 of D-CUT: field/const/branch cleanup after the 34 functions are gone.
[CmdletBinding()]
param([switch]$Apply, [string]$Path = 'D:\ModeChat\src\main.rs')
$ErrorActionPreference = 'Stop'
$L = [System.IO.File]::ReadAllLines($Path, [System.Text.Encoding]::UTF8)
Write-Output ("before: {0} lines" -f $L.Count)

# 1-based inclusive ranges to drop, verified by content (checked below)
$ranges = @(
  @(187, 187, 'const SIDEBAR_WIDTH: f32 = 280.0;'),
  @(243, 244, 'const BUBBLE_MAX_WIDTH: f32 = 672.0;'),
  @(3358, 3359, 'wecom_mode: bool,'),
  @(3445, 3445, 'active_tab: usize,'),
  @(3830, 3830, 'wecom_mode: std::env::var('),
  @(3888, 3888, 'active_tab: 0,'),
  @(3908, 3908, 'self.active_tab = 0;'),
  @(4169, 4169, 'self.active_tab = 0;'),
  @(7357, 7357, 'self.active_tab = 0;'),
  @(11459, 11459, 'self.active_tab = 0;'),
  @(17055, 17055, 'self.active_tab = 0;'),
  @(11764, 11766, 'if !self.wecom_mode'),
  @(26233, 26235, 'BOT_PILOT_CLASSIC_UI'),
  @(26478, 26488, 'mod composer_tests')
)

$bad = 0
foreach ($r in $ranges) {
  $s = $r[0]; $e = $r[1]; $needle = $r[2]
  $joined = ($L[($s-1)..($e-1)] -join "`n")
  if (-not $joined.Contains($needle)) {
    Write-Output ("ANCHOR FAIL {0}-{1} expect '{2}'" -f $s, $e, $needle)
    $bad++
  }
}
if ($bad -gt 0) { Write-Output ("$bad anchor(s) failed -- nothing written"); exit 1 }
Write-Output ("all {0} ranges verified by content" -f $ranges.Count)

if (-not $Apply) { Write-Output 'DRY RUN'; exit 0 }

$drop = New-Object bool[] $L.Count
foreach ($r in $ranges) { for ($i = $r[0] - 1; $i -le $r[1] - 1; $i++) { $drop[$i] = $true } }
$kept = New-Object System.Collections.Generic.List[string]
for ($i = 0; $i -lt $L.Count; $i++) { if (-not $drop[$i]) { $kept.Add($L[$i]) } }

$out = New-Object System.Collections.Generic.List[string]
$blank = 0
foreach ($l in $kept) {
  if ($l.Trim().Length -eq 0) { $blank++; if ($blank -gt 1) { continue } } else { $blank = 0 }
  $out.Add($l)
}
$text = ($out -join "`r`n") + "`r`n"
[System.IO.File]::WriteAllText($Path, $text, (New-Object System.Text.UTF8Encoding($false)))
Write-Output ("WROTE {0}  {1} -> {2}" -f $Path, $L.Count, $out.Count)
