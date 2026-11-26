# 把「行号 + 纯标识符标签」的锚点改成纯符号锚（去掉行号）。
#
# 为什么：`file:23642`（`chat_info_panel`）这种写法虽然能过 symbol-nearby 校验，
# 但**它仍然依赖行号** —— 下次 src 一动就漂。标签在文件里唯一命中时，行号是多余的。
#
# 判据：标签（去掉 fn 前缀）在源码里恰好出现 1 次才转；否则保留原样并报出来。
param([switch]$Apply)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$docs = @('docs/parity-matrix.md')

$rx = [regex]'`(src/[\w/\.]+\.rs):(\d+)`(\s*[（(]\s*`([^`]{2,90})`)?'
$cache = @{}
function Get-Lines([string]$rel) {
  if ($cache.ContainsKey($rel)) { return $cache[$rel] }
  $full = Join-Path $root $rel
  if (-not (Test-Path $full)) { $cache[$rel] = $null; return $null }
  $cache[$rel] = [System.IO.File]::ReadAllLines($full)
  return $cache[$rel]
}
function Count-Hits([string]$rel, [string]$needle) {
  $lines = Get-Lines $rel
  if (-not $lines) { return 0 }
  $n = 0
  foreach ($l in $lines) { if ($l.Contains($needle)) { $n++ } }
  return $n
}

$bt = [char]0x60
$converted = New-Object System.Collections.Generic.List[string]
$kept = New-Object System.Collections.Generic.List[string]

foreach ($doc in $docs) {
  $full = Join-Path $root $doc
  if (-not (Test-Path $full)) { continue }
  $lines = [System.IO.File]::ReadAllLines($full, [System.Text.Encoding]::UTF8)
  $out = New-Object System.Collections.Generic.List[string]
  $ln = 0
  $changed = 0
  foreach ($line in $lines) {
    $ln++
    $new = $line
    foreach ($m in ($rx.Matches($line) | Sort-Object -Property Index -Descending)) {
      $rel = $m.Groups[1].Value.Trim()
      $num = $m.Groups[2].Value
      if (-not $m.Groups[4].Success) { continue }
      $lab = $m.Groups[4].Value.Trim()
      # 只处理「纯标识符」或「fn 标识符」，带括号/中文/引号的标签不动
      if ($lab -notmatch '^(fn\s+)?[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)*$') { continue }
      $needle = $lab -replace '^fn\s+', ''
      $hits = Count-Hits $rel $needle
      if ($hits -ne 1) {
        $kept.Add("$doc`:$ln  $rel`:$num [$lab] 命中 $hits 次")
        continue
      }
      $repl = "$bt$lab$bt"
      $new = $new.Substring(0, $m.Index) + $repl + $new.Substring($m.Index + $m.Length)
      $converted.Add("$doc`:$ln  $rel`:$num -> $repl")
      $changed++
    }
    $out.Add($new)
  }
  if ($Apply -and $changed -gt 0) {
    [System.IO.File]::WriteAllLines($full, $out, (New-Object System.Text.UTF8Encoding($false)))
  }
  Write-Output ("{0,-24} 转成纯符号锚 {1,3} 条" -f $doc, $changed)
}
Write-Output ''
Write-Output "converted: $($converted.Count)   kept: $($kept.Count)   mode: $(if($Apply){'APPLY'}else{'dry-run'})"
if ($kept.Count -gt 0) { Write-Output ''; Write-Output '== 保留行号（标签不唯一或非纯标识符）=='; foreach ($k in $kept) { Write-Output "  $k" } }
