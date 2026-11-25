# UI design-token gate (Q02 / PRD P0-6 AC5)
#
# Default "report mode": prints violations, never changes exit code -- used while
# the frontend cleans up (W10). After cleanup, CI calls it with -Enforce, so any
# violation fails the release.
#
# Four rules (see docs/ui-design-spec.md section 10):
#   1) .cursor_pointer()      only inside src/ui_kit.rs
#   2) .rounded(px(<literal>)) only inside src/ui_kit.rs, src/theme.rs
#      Note: the argument must be a NUMERIC LITERAL to count. Passing a token
#      (theme::RADIUS_*, ui::R_CARD) or a computed value (avatar radius,
#      composer_radius) is the intended usage and is NOT a violation --
#      spec section 10 item 1 already closed that debt.
#   3) text_size(px(N))       N must be one of the four font-size tokens
#   4) rgb(...)               only inside the theme / icon-tint whitelist
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\ui-lint.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\ui-lint.ps1 -Enforce
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\ui-lint.ps1 -Top 5

[CmdletBinding()]
param(
  [switch]$Enforce,
  [int]$Top = 8,
  [string]$Root = ''
)

$ErrorActionPreference = 'Stop'

if ($Root -eq '') {
  $Root = Join-Path (Split-Path -Parent $PSCommandPath) '..\src'
}

# Font-size tokens from src/ui_kit.rs: T_LABEL 11 / T_BODY 12 / T_TITLE 13 /
# T_PAGE 15 / T_METRIC 22. Writing one of these numbers literally is a violation.
$FontTokens = @('11.', '12.', '13.', '15.', '22.')

$CursorAllow = @('ui_kit.rs')
$RadiusAllow = @('ui_kit.rs', 'theme.rs')
# Brand-tint icon files may use rgb() (docs/ui-design-spec.md 10.1).
$RgbAllow = @(
  'theme.rs', 'wechat_theme.rs', 'agent_icons.rs', 'bot_icon.rs',
  'brand_icons.rs', 'avatar.rs', 'menu_icons.rs', 'icons.rs'
)

$files = @(Get-ChildItem -Path $Root -Recurse -File -Include *.rs)
if ($files.Count -eq 0) { throw "no .rs files under: $Root" }

$all = New-Object System.Collections.ArrayList

foreach ($f in $files) {
  $name = $f.Name
  $ln = 0
  foreach ($line in [System.IO.File]::ReadLines($f.FullName)) {
    $ln++
    $lead = $line.TrimStart()
    if ($lead.StartsWith('//') -or $lead.StartsWith('*')) { continue }

    if ($line -match '\.cursor_pointer\(\)' -and $CursorAllow -notcontains $name) {
      [void]$all.Add([pscustomobject]@{ Rule = 'cursor_pointer'; File = $name; Line = $ln })
    }
    if ($line -match '\.rounded\(px\([0-9]' -and $RadiusAllow -notcontains $name) {
      [void]$all.Add([pscustomobject]@{ Rule = 'rounded_px'; File = $name; Line = $ln })
    }
    if ($line -match 'text_size\(px\(([0-9]+(?:\.[0-9]+)?)\.?\)') {
      $value = $Matches[1] + '.'
      if ($FontTokens -notcontains $value) {
        [void]$all.Add([pscustomobject]@{ Rule = 'text_size_offtoken'; File = $name; Line = $ln })
      }
    }
    if ($line -match 'rgb\(' -and $RgbAllow -notcontains $name) {
      [void]$all.Add([pscustomobject]@{ Rule = 'rgb'; File = $name; Line = $ln })
    }
  }
}

$labels = @{
  cursor_pointer     = '.cursor_pointer()'
  rounded_px         = '.rounded(px(...))'
  text_size_offtoken = 'text_size(px(non-token))'
  rgb                = 'rgb(...)'
}
$order = @('cursor_pointer', 'rounded_px', 'text_size_offtoken', 'rgb')

$total = $all.Count
Write-Output ''
Write-Output ("UI token gate  root: {0}  files: {1}" -f (Resolve-Path $Root).Path, $files.Count)
Write-Output ('-' * 72)

foreach ($key in $order) {
  $hits = @($all | Where-Object { $_.Rule -eq $key })
  $status = 'OK  '
  if ($hits.Count -gt 0) { $status = 'FAIL' }
  Write-Output ("[{0}] {1,-28} {2,5}" -f $status, $labels[$key], $hits.Count)
  if ($hits.Count -gt 0 -and $Top -gt 0) {
    $byFile = $hits | Group-Object File | Sort-Object Count -Descending | Select-Object -First $Top
    foreach ($g in $byFile) {
      Write-Output ("         {0,-24} {1,4}" -f $g.Name, $g.Count)
    }
  }
}

Write-Output ('-' * 72)
Write-Output ("total violations: {0}" -f $total)
Write-Output ''

if ($Enforce -and $total -gt 0) {
  Write-Output 'RESULT: FAIL (-Enforce, exit 1)'
  exit 1
}
if ($Enforce) {
  Write-Output 'RESULT: PASS'
  exit 0
}
Write-Output 'RESULT: report-only (no -Enforce, exit 0)'
exit 0
