# After a mechanical remap, some doc anchors land on blank / structural lines
# (a line containing only brackets or punctuation). Snap those back to the nearest
# line within +-12 that actually carries one of the symbols named on the doc line.
# +-12 is the tolerance the label check already uses, so this never invents a new target.
[CmdletBinding()]
param([switch]$Apply)
$ErrorActionPreference = 'Stop'
$root = 'D:\ModeChat'
$src = [System.IO.File]::ReadAllLines((Join-Path $root 'src\main.rs'), [System.Text.Encoding]::UTF8)

function Is-Bad([string]$t) {
  if ($null -eq $t) { return $true }
  $s = $t.Trim()
  if ($s.Length -eq 0) { return $true }
  return ($s -match '^[(){}\[\];,\.]+$')
}

function Syms([string]$line) {
  $out = New-Object System.Collections.Generic.List[string]
  foreach ($m in [regex]::Matches($line, '`([^`]+)`')) {
    $inner = $m.Groups[1].Value
    foreach ($mm in [regex]::Matches($inner, 'fn\s+([a-z0-9_]+)')) { $out.Add($mm.Groups[1].Value) }
    foreach ($mm in [regex]::Matches($inner, '"([^"]+)"')) { $out.Add($mm.Groups[1].Value) }
    foreach ($mm in [regex]::Matches($inner, '^([a-z][a-z0-9_]{3,})$')) { $out.Add($mm.Groups[1].Value) }
  }
  return $out
}

$docs = @('docs/parity-matrix.md', 'docs/prd-0.5.md', 'docs/prd-0.6.md', 'docs/parity-arch-notes.md', 'docs/plans/2026-11-25-v0.5.0.md')
$fixed = 0; $left = 0
foreach ($rel in $docs) {
  $p = Join-Path $root $rel
  if (-not (Test-Path $p)) { continue }
  $lines = [System.IO.File]::ReadAllLines($p, [System.Text.Encoding]::UTF8)
  $changed = $false
  for ($i = 0; $i -lt $lines.Count; $i++) {
    $matches = [regex]::Matches($lines[$i], '(src/main\.rs:(\d+)|:(\d+))')
    foreach ($h in $matches) {
      $n = 0
      if ($h.Groups[2].Success) { $n = [int]$h.Groups[2].Value } else { $n = [int]$h.Groups[3].Value }
      if ($n -lt 1 -or $n -gt $src.Count) { continue }
      if (-not (Is-Bad $src[$n-1])) { continue }
      $syms = Syms $lines[$i]
      $best = 0; $bestDist = 99
      for ($d = 1; $d -le 12; $d++) {
        foreach ($cand in @(($n - $d), ($n + $d))) {
          if ($cand -lt 1 -or $cand -gt $src.Count) { continue }
          if (Is-Bad $src[$cand-1]) { continue }
          foreach ($s in $syms) {
            if ($src[$cand-1] -match [regex]::Escape($s)) { if ($d -lt $bestDist) { $best = $cand; $bestDist = $d } }
          }
        }
        if ($best -gt 0) { break }
      }
      if ($best -eq 0) {
        Write-Output ("LEFT {0}:{1} :{2}" -f $rel, ($i+1), $n)
        $left++
        continue
      }
      $old = $h.Value
      $new = $old.Replace(":$n", ":$best")
      $lines[$i] = $lines[$i].Replace($old, $new)
      Write-Output ("SNAP {0}:{1} {2} -> {3} (d={4})" -f $rel, ($i+1), $old, $new, $bestDist)
      $fixed++; $changed = $true
    }
  }
  if ($changed -and $Apply) {
    [System.IO.File]::WriteAllText($p, (($lines -join "`r`n") + "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
  }
}
Write-Output ("snapped={0} left={1} apply={2}" -f $fixed, $left, $Apply)
