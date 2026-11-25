# Verify every `file:line` anchor written in docs/parity-arch-notes.md.
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify-arch-anchors.ps1
$ErrorActionPreference = 'Stop'

$doc   = 'D:\ModeChat\docs\parity-arch-notes.md'
$roots = @{
  mc  = 'D:\ModeChat'
  ds  = 'D:\Mode Bot\DS-Bot'
  rpi = 'C:\Users\Administrator\.cargo\git\checkouts\pi-rust-ba8012a5f17fe953\29d8686\crates'
}

function Resolve-Path2([string]$file) {
  if ($file -like 'ds-bot/*' -or $file -like 'link/*' -or $file -like 'app/*' -or $file -eq 'README.zh.md') {
    return (Join-Path $roots.ds $file)
  }
  if ($file -like 'rpi-harness/*') { return (Join-Path $roots.rpi $file) }
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
  if (-not (Test-Path $path)) { continue }
  $lines = [System.IO.File]::ReadAllLines($path)
  $lo = [Math]::Max(1, $n - 20); $hi = [Math]::Min($lines.Count, $n + 20)
  $hit = $false
  for ($i = $lo; $i -le $hi; $i++) { if ($lines[$i-1].Contains($sym)) { $hit = $true; break } }
  $nearChecked++
  if (-not $hit) { $bad.Add("SYMBOL DRIFT $f`:$n 附近无 ``$sym``") }
}
Write-Output ''
Write-Output ("anchors={0}  ok={1}  bad={2}  symbol-checked={3}" -f $pairs.Count, $ok, $bad.Count, $nearChecked)
foreach ($b in $bad) { Write-Output "BAD $b" }
if ($bad.Count -gt 0) { exit 1 }
