# Step 3 of D-CUT: theme.rs dead functions + cascading constants.
[CmdletBinding()]
param([switch]$Apply, [string]$Path = 'D:\ModeChat\src\theme.rs')
$ErrorActionPreference = 'Stop'
$T = [System.IO.File]::ReadAllLines($Path, [System.Text.Encoding]::UTF8)
Write-Output ("before: {0} lines" -f $T.Count)

# @(start, end, expected-content-needle) 1-based inclusive
$ranges = @(
  @(142, 147, 'pub fn set_light(light: bool) -> bool {'),
  @(227, 229, 'pub fn hairline() -> Rgba {'),
  @(258, 267, 'pub fn ink_line() -> Rgba {'),
  @(292, 295, 'pub fn subtle_hover() -> Rgba {'),
  @(339, 341, 'pub fn accent_border() -> Rgba {'),
  @(405, 407, 'pub fn chief() -> Rgba {'),
  @(411, 413, 'pub fn bubble_user() -> Rgba {'),
  @(417, 419, 'pub fn bubble_assistant() -> Rgba {'),
  @(441, 444, 'pub fn member_color(id: &str) -> Rgba {'),
  @(466, 472, 'pub fn shadow_1() -> Vec<BoxShadow> {'),
  @(550, 552, 'pub const RADIUS_COMPOSER: f32 = 22.0;'),
  @(636, 636, 'assert_eq!(shadow_1().len(), 2);')
)

$bad = 0
foreach ($r in $ranges) {
  $joined = ($T[($r[0]-1)..($r[1]-1)] -join "`n")
  if (-not $joined.Contains($r[2])) {
    Write-Output ("ANCHOR FAIL {0}-{1} expect '{2}'" -f $r[0], $r[1], $r[2])
    $bad++
  }
}
if ($bad -gt 0) { Write-Output ("$bad failed -- nothing written"); exit 1 }
Write-Output ("all {0} ranges verified" -f $ranges.Count)

if (-not $Apply) { Write-Output 'DRY RUN'; exit 0 }

$drop = New-Object bool[] $T.Count
foreach ($r in $ranges) { for ($i = $r[0]-1; $i -le $r[1]-1; $i++) { $drop[$i] = $true } }
$kept = New-Object System.Collections.Generic.List[string]
for ($i = 0; $i -lt $T.Count; $i++) { if (-not $drop[$i]) { $kept.Add($T[$i]) } }
$out = New-Object System.Collections.Generic.List[string]
$blank = 0
foreach ($l in $kept) {
  if ($l.Trim().Length -eq 0) { $blank++; if ($blank -gt 1) { continue } } else { $blank = 0 }
  $out.Add($l)
}
$text = ($out -join "`r`n") + "`r`n"
[System.IO.File]::WriteAllText($Path, $text, (New-Object System.Text.UTF8Encoding($false)))
Write-Output ("WROTE {0}  {1} -> {2}" -f $Path, $T.Count, $out.Count)
