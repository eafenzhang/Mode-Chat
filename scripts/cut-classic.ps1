# Delete classic-shell functions from src/main.rs by NAME (brace-accurate), not by line range.
# Dry run by default; pass -Apply to write. Keeps file as UTF-8 (no BOM) + CRLF.
[CmdletBinding()]
param(
  [switch]$Apply,
  [string]$Path = 'D:\ModeChat\src\main.rs'
)

$ErrorActionPreference = 'Stop'

# name + whether it is an inherent method (indent 4) or a free fn (indent 0)
$targets = @(
  @('classic_shell', 4), @('titlebar', 4), @('window_button', 4), @('sidebar', 4),
  @('main_bot_card', 4), @('unread_badge', 4), @('chat_header', 4), @('transcript', 4),
  @('composer', 4), @('right_panel', 4), @('status_color', 4), @('control_card', 4),
  @('chat_status', 4), @('settings_overlay', 4), @('theme_picker', 4), @('shell', 4),
  @('bot_details', 0), @('group_details', 0), @('group_placeholder', 0), @('responder_label', 0),
  @('short_session', 0), @('composer_radius', 0), @('icon_button', 0), @('day_separator', 0),
  @('msg_time', 0), @('load_earlier_row', 0), @('message_row', 0), @('quote_button', 0),
  @('copy_button', 0), @('message_actions', 0), @('message_attachments', 0), @('two_line', 0),
  @('text_block', 0), @('tool_card', 0)
)

$lines = [System.IO.File]::ReadAllLines($Path, [System.Text.Encoding]::UTF8)
Write-Output ("source: {0}  lines: {1}" -f $Path, $lines.Count)

# strip string/char literals so braces inside them do not skew the counter
function Strip-Literals([string]$s) {
  $out = New-Object System.Text.StringBuilder
  $i = 0
  $inStr = $false
  while ($i -lt $s.Length) {
    $c = $s[$i]
    if ($inStr) {
      if ($c -eq '\') { $i += 2; continue }
      if ($c -eq '"') { $inStr = $false }
      $i++
      continue
    }
    if ($c -eq '"') { $inStr = $true; $i++; continue }
    if ($c -eq '/' -and ($i + 1) -lt $s.Length -and $s[$i + 1] -eq '/') { break }
    [void]$out.Append($c)
    $i++
  }
  return $out.ToString()
}

function Find-End([int]$startIdx) {
  $depth = 0
  $opened = $false
  for ($i = $startIdx; $i -lt $lines.Count; $i++) {
    $t = Strip-Literals $lines[$i]
    $depth += ([regex]::Matches($t, '\{')).Count
    $depth -= ([regex]::Matches($t, '\}')).Count
    if ($depth -gt 0) { $opened = $true }
    elseif ($opened) { return $i }
  }
  return -1
}

$found = @()
foreach ($t in $targets) {
  $name = $t[0]
  $indent = $t[1]
  $rx = '^' + (' ' * $indent) + 'fn ' + [regex]::Escape($name) + '\b'
  $hits = @()
  for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($lines[$i] -match $rx) { $hits += $i }
  }
  if ($hits.Count -eq 0) { Write-Output ("MISS    {0}" -f $name); continue }
  if ($hits.Count -gt 1) { Write-Output ("AMBIG   {0} -> {1}" -f $name, (($hits | ForEach-Object { $_ + 1 }) -join ',')); continue }
  $fnLine = $hits[0]
  $end = Find-End $fnLine
  if ($end -lt 0) { Write-Output ("BADEND  {0}" -f $name); continue }
  # absorb contiguous doc comments right above
  $start = $fnLine
  while ($start -gt 0 -and $lines[$start - 1] -match '^\s*///') { $start-- }
  $found += [pscustomobject]@{ Name = $name; Start = $start; End = $end; Lines = ($end - $start + 1) }
}

$found = $found | Sort-Object Start
$total = 0
Write-Output ''
Write-Output 'name                     from    to    lines   first / last'
foreach ($f in $found) {
  $total += $f.Lines
  $first = $lines[$f.Start].Trim()
  $last = $lines[$f.End].Trim()
  if ($first.Length -gt 46) { $first = $first.Substring(0, 46) }
  Write-Output ("{0,-22} {1,6} {2,5} {3,7}   {4} ... {5}" -f $f.Name, ($f.Start + 1), ($f.End + 1), $f.Lines, $first, $last)
}
Write-Output ''
Write-Output ("resolved {0}/{1} symbols, {2} lines total" -f $found.Count, $targets.Count, $total)

# overlap check
for ($i = 1; $i -lt $found.Count; $i++) {
  if ($found[$i].Start -le $found[$i - 1].End) {
    Write-Output ("OVERLAP {0} with {1}" -f $found[$i].Name, $found[$i - 1].Name)
  }
}

if (-not $Apply) {
  Write-Output ''
  Write-Output 'DRY RUN (no file written)'
  exit 0
}

# delete bottom-up
$drop = New-Object bool[] $lines.Count
foreach ($f in $found) { for ($i = $f.Start; $i -le $f.End; $i++) { $drop[$i] = $true } }
$kept = New-Object System.Collections.Generic.List[string]
for ($i = 0; $i -lt $lines.Count; $i++) { if (-not $drop[$i]) { $kept.Add($lines[$i]) } }

# collapse runs of 3+ blank lines to 1 (rustfmt upper bound is 1)
$out = New-Object System.Collections.Generic.List[string]
$blank = 0
foreach ($l in $kept) {
  if ($l.Trim().Length -eq 0) { $blank++; if ($blank -gt 1) { continue } } else { $blank = 0 }
  $out.Add($l)
}
$text = ($out -join "`r`n") + "`r`n"
[System.IO.File]::WriteAllText($Path, $text, (New-Object System.Text.UTF8Encoding($false)))
Write-Output ''
Write-Output ("WROTE {0}  lines {1} -> {2}" -f $Path, $lines.Count, $out.Count)
