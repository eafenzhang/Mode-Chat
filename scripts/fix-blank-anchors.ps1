# Fix doc anchors that landed on blank/structural lines, by re-deriving the target line from
# the symbol names written in backticks on the same doc line.
[CmdletBinding()]
param([switch]$Apply)
$ErrorActionPreference = 'Stop'
$root = 'D:\ModeChat'
$src = [System.IO.File]::ReadAllLines((Join-Path $root 'src\main.rs'), [System.Text.Encoding]::UTF8)

function Is-Bad([string]$text) {
  if ($text -eq $null) { return $true }
  $t = $text.Trim()
  if ($t.Length -eq 0) { return $true }
  return ($t -match '^[(){}\[\];,\.]+$')
}

# candidate symbols on a doc line: `fn x` / `x` inside backticks / `.id("x")` / ids
function Symbols-Of([string]$line) {
  $out = New-Object System.Collections.Generic.List[string]
  foreach ($m in [regex]::Matches($line, '`([^`]+)`')) {
    $inner = $m.Groups[1].Value
    foreach ($mm in [regex]::Matches($inner, 'fn\s+([a-z0-9_]+)')) { $out.Add($mm.Groups[1].Value) }
    foreach ($mm in [regex]::Matches($inner, '"([a-zA-Z0-9_\-]+)"')) { $out.Add($mm.Groups[1].Value) }
    if ($inner -match '^[a-z][a-z0-9_]{3,}$') { $out.Add($inner) }
  }
  return $out
}

function Find-Line([string]$sym) {
  $pat = 'fn\s+' + [regex]::Escape($sym) + '\b'
  for ($i = 0; $i -lt $src.Count; $i++) { if ($src[$i] -match $pat) { return $i + 1 } }
  $pat2 = [regex]::Escape($sym)
  for ($i = 0; $i -lt $src.Count; $i++) { if ($src[$i] -match $pat2) { return $i + 1 } }
  return 0
}

$docs = @('docs/parity-matrix.md', 'docs/prd-0.5.md', 'docs/prd-0.6.md', 'docs/parity-arch-notes.md')
$fixed = 0; $unfixed = 0
foreach ($rel in $docs) {
  $p = Join-Path $root $rel
  if (-not (Test-Path $p)) { continue }
  $lines = [System.IO.File]::ReadAllLines($p, [System.Text.Encoding]::UTF8)
  $changed = $false
  for ($i = 0; $i -lt $lines.Count; $i++) {
    $hits = [regex]::Matches($lines[$i], 'src/main\.rs:(\d+)')
    if ($hits.Count -eq 0) { continue }
    foreach ($h in $hits) {
      $n = [int]$h.Groups[1].Value
      if ($n -lt 1 -or $n -gt $src.Count) { Write-Output ("OOB {0}:{1} :{2}" -f $rel, ($i+1), $n); $unfixed++; continue }
      if (-not (Is-Bad $src[$n-1])) { continue }
      $syms = Symbols-Of $lines[$i]
      $new = 0
      foreach ($s in $syms) { $c = Find-Line $s; if ($c -gt 0 -and -not (Is-Bad $src[$c-1])) { $new = $c; break } }
      if ($new -eq 0) {
        Write-Output ("UNFIXED {0}:{1} :{2} syms=[{3}]" -f $rel, ($i+1), $n, ($syms -join ','))
        $unfixed++
        continue
      }
      $lines[$i] = $lines[$i].Replace("src/main.rs:$n", "src/main.rs:$new")
      Write-Output ("FIX {0}:{1} :{2} -> :{3}" -f $rel, ($i+1), $n, $new)
      $fixed++; $changed = $true
    }
  }
  if ($changed -and $Apply) {
    [System.IO.File]::WriteAllText($p, (($lines -join "`r`n") + "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
  }
}
Write-Output ("fixed={0} unfixed={1} apply={2}" -f $fixed, $unfixed, $Apply)
