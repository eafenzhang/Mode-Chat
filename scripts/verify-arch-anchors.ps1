# Verify every `file:line` anchor written in docs/parity-arch-notes.md.
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify-arch-anchors.ps1
#
# Roots are resolved from this script's location so the same script works in CI
# (a fresh checkout under a runner temp dir) and locally. The two *external*
# reference trees — the DS-Bot baseline and the pi-rust checkout — are optional:
# set MODE_PARITY_DS_ROOT / MODE_PARITY_RPI_ROOT to check them too. Anchors that
# point into an unavailable external tree are skipped and counted, never silently
# treated as OK for the repo's own files.
$ErrorActionPreference = 'Stop'

$mc    = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$ds    = if ($env:MODE_PARITY_DS_ROOT)  { $env:MODE_PARITY_DS_ROOT }  else { '' }
$rpi   = if ($env:MODE_PARITY_RPI_ROOT) { $env:MODE_PARITY_RPI_ROOT } else { '' }
$doc   = Join-Path $mc 'docs/parity-arch-notes.md'
$roots = @{ mc = $mc; ds = $ds; rpi = $rpi }

$skippedExternal = 0

function Resolve-Path2([string]$file) {
  if ($file -like 'ds-bot/*' -or $file -like 'link/*' -or $file -like 'app/*' -or $file -eq 'README.zh.md') {
    if (-not $roots.ds) { return $null }
    return (Join-Path $roots.ds $file)
  }
  if ($file -like 'rpi-harness/*' -or $file -like 'rpi-*/*') {
    if (-not $roots.rpi) { return $null }
    return (Join-Path $roots.rpi $file)
  }
  return (Join-Path $roots.mc $file)
}

$text  = [System.IO.File]::ReadAllText($doc)
$hits  = [regex]::Matches($text, '([A-Za-z0-9_\-/\.]+\.(?:rs|js|md|ts)):(\d+)')
$pairs = $hits | ForEach-Object { "$($_.Groups[1].Value):$($_.Groups[2].Value)" } | Sort-Object -Unique

$ok = 0
$bad = New-Object System.Collections.Generic.List[string]
foreach ($pair in $pairs) {
  $file = ($pair -split ':')[0]
  $n    = [int](($pair -split ':')[1])
  $path = Resolve-Path2 $file
  if (-not $path) { $skippedExternal++; continue }
  if (-not (Test-Path $path)) { $bad.Add("$pair  [file not found: $path]"); continue }
  $lines = [System.IO.File]::ReadAllLines($path)
  if ($n -lt 1 -or $n -gt $lines.Count) { $bad.Add("$pair  [out of range, file has $($lines.Count)]"); continue }
  $line = $lines[$n-1].Trim()
  if ($line -eq '') { $bad.Add("$pair  [blank line]"); continue }
  Write-Output ("{0}:{1} -> {2}" -f $file, $n, $line)
  $ok++
}

# 符号就近校验：doc 里写成 `file:N`（symbol） 的，N±20 行内必须出现该符号。
$nearChecked = 0
foreach ($m in [regex]::Matches($text, '`([A-Za-z0-9_\-/\.]+\.(?:rs|js|md)):(\d+)`\s*[（(]\s*([^）)]+)[）)]')) {
  $f = $m.Groups[1].Value
  $n = [int]$m.Groups[2].Value
  $sym = $m.Groups[3].Value.Trim()
  if ($sym -match '[{},\*<>]') { continue }
  $sym = ($sym -split '::')[-1].TrimEnd('(', '{', ':', ' ')
  # 只校验「看起来是标识符」的括号内容；中文说明/短语跳过
  $sym = $sym -replace '`',''
  if ($sym -notmatch '^[A-Za-z_][A-Za-z0-9_]*$') { continue }
  if ($sym.Length -lt 4) { continue }
  $path = Resolve-Path2 $f
  if (-not $path) { continue }
  if (-not (Test-Path $path)) { continue }
  $lines = [System.IO.File]::ReadAllLines($path)
  $lo = [Math]::Max(1, $n - 20); $hi = [Math]::Min($lines.Count, $n + 20)
  $hit = $false
  for ($i = $lo; $i -le $hi; $i++) { if ($lines[$i-1].Contains($sym)) { $hit = $true; break } }
  $nearChecked++
  if (-not $hit) { $bad.Add("SYMBOL DRIFT $f`:$n 附近无 ``$sym``") }
}
Write-Output ""
Write-Output ("anchors={0}  ok={1}  bad={2}  symbol-checked={3}  skipped-external={4}" -f $pairs.Count, $ok, $bad.Count, $nearChecked, $skippedExternal)
foreach ($b in $bad) { Write-Output "BAD $b" }
# 判决行：没有它只有读数。人扫一眼 bad=0 就以为通过，而 skipped-external 那部分
# 既不在 ok 里也不在 bad 里。措辞里必须带上 skipped，否则 anchors 与 ok 的差额没人看。
if ($bad.Count -gt 0) {
  Write-Output ("RESULT: FAIL ({0} bad)" -f $bad.Count)
  exit 1
}
Write-Output ("RESULT: all arch anchors resolve ({0} anchors, {1} ok, {2} skipped-external not verifiable here)" -f $pairs.Count, $ok, $skippedExternal)
exit 0
