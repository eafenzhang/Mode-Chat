# Step 4 of D-CUT: pure-dead UI helpers left behind by the classic deletion.
# Every range is verified by content before anything is written.
[CmdletBinding()]
param([switch]$Apply)
$ErrorActionPreference = 'Stop'

function Cut([string]$path, $ranges) {
  $L = [System.IO.File]::ReadAllLines($path, [System.Text.Encoding]::UTF8)
  $bad = 0
  foreach ($r in $ranges) {
    $joined = ($L[($r[0]-1)..($r[1]-1)] -join "`n")
    if (-not $joined.Contains($r[2])) { Write-Output ("ANCHOR FAIL {0} {1}-{2} '{3}'" -f $path, $r[0], $r[1], $r[2]); $bad++ }
  }
  if ($bad -gt 0) { return ("FAILED {0} -- nothing written" -f $path) }
  if (-not $Apply) { return ("DRY {0}: {1} ranges ok, currently {2} lines" -f $path, $ranges.Count, $L.Count) }
  $drop = New-Object bool[] $L.Count
  foreach ($r in $ranges) { for ($i = $r[0]-1; $i -le $r[1]-1; $i++) { $drop[$i] = $true } }
  $kept = New-Object System.Collections.Generic.List[string]
  for ($i = 0; $i -lt $L.Count; $i++) { if (-not $drop[$i]) { $kept.Add($L[$i]) } }
  $out = New-Object System.Collections.Generic.List[string]
  $blank = 0
  foreach ($l in $kept) { if ($l.Trim().Length -eq 0) { $blank++; if ($blank -gt 1) { continue } } else { $blank = 0 }; $out.Add($l) }
  [System.IO.File]::WriteAllText($path, (($out -join "`r`n") + "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
  return ("WROTE {0}  {1} -> {2}" -f $path, $L.Count, $out.Count)
}

# main.rs: three helpers whose only callers were classic
Write-Output (Cut 'D:\ModeChat\src\main.rs' @(
  @(1003, 1017, 'fn sidebar_status_badge(status: &str)'),
  @(20331, 20345, 'fn nav_row(id: impl Into<gpui::ElementId>'),
  @(20351, 20383, 'trait NavRowFocus: Sized'),
  @(22019, 22026, 'fn tool_state_color(state: &str)')
))

# avatar.rs: hue-gradient avatar stack (only classic used it)
Write-Output (Cut 'D:\ModeChat\src\avatar.rs' @(
  @(37, 39, 'fn deg(h: f32, s: f32, l: f32, a: f32) -> Hsla'),
  @(41, 77, 'fn shell(hue: f32, width: f32, height: f32, radius: f32)'),
  @(196, 213, 'pub fn bot_avatar(id: &str, size: f32) -> AnyElement'),
  @(239, 254, 'pub fn channel_avatar(id: &str, size: f32) -> impl IntoElement'),
  @(255, 265, 'pub fn unread_dot(size: f32) -> impl IntoElement')
))
