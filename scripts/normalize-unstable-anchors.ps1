# 把「指向空行 / 纯括号行」的锚点从行号锚改成符号锚，或在无符号可依时降级为纯文本。
#
# 为什么：`src/main.rs` 两万六千行，每加一行，后面所有行号全部失效。这一轮
# 反复出现同一批锚点「修一轮错一轮」，根因不是改得不对，是**行号本身不能当锚**：
# 它只是「当时的行号」，不是「那句话」。18.3（重复行假一致）与 18.10（区间终点
# 被结构校验豁免）都是这个根因的两种表现。
#
# 所以对「已经指到空行/纯括号行」的锚点：
#   ① 同行括号里有能在文件里唯一定位的符号 → 改成 `file:符号所在行`（保留防漂移能力）
#   ② 找不到唯一符号                      → 去掉 `:行号`，只留 `file`（或裸行号整段去掉），
#      文字叙述保留。这类锚点本来就没有保护能力，留着只会制造 STRUCTURAL/EMPTY 噪声。
#
# 只动「已经坏掉」的锚点，指对了一律不碰。
param([switch]$Apply)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$docs = @('docs/parity-matrix.md', 'docs/prd-0.5.md', 'docs/prd-0.6.md')

$fullRx = [regex]'`([A-Za-z0-9_./\\-]+\.(?:rs|js|mjs)):(\d+)`'
$bareRx = [regex]'`:(\d+)`'
$fileOnLineRx = [regex]'`([A-Za-z0-9_./\\-]+\.(?:rs|js|mjs)):'
# 锚点之后紧跟的括号标签（可能隔一个空格）
$labelAfterRx = [regex]'^\s*[（(]\s*`([^`]{1,160})`'

$cache = @{}
function Get-Lines([string]$rel) {
  if ($cache.ContainsKey($rel)) { return $cache[$rel] }
  $full = Join-Path $root $rel
  if (-not (Test-Path $full)) { $cache[$rel] = $null; return $null }
  $cache[$rel] = [System.IO.File]::ReadAllLines($full)
  return $cache[$rel]
}
function Is-Bad([string]$line) {
  $t = $line.Trim()
  if ($t -eq '') { return $true }
  return ($t -match '^[\(\)\[\]\{\};,]+$')
}
function Find-Unique([string]$rel, [string]$label) {
  $lines = Get-Lines $rel
  if (-not $lines) { return -1 }
  $raw = @()
  # ① 整条标签当字面量（去掉 trailing 的 -*/…、去掉 "fn " 前缀）——标签本身就是源码片段
  $lit = $label.Trim()
  foreach ($v in @($lit, ($lit -replace '^fn\s+', ''), ($lit -replace '-\*$', '-'), ($lit -replace '[（(].*$', '').Trim())) {
    if ($v -and $v.Length -ge 5 -and $v.Length -le 90 -and $v -notmatch '\s{2,}') { $raw += $v }
  }
  # ② 双引号里的字符串，按去引号后的字面搜
  foreach ($m in [regex]::Matches($label, '"([^"]{4,})"')) { $raw += $m.Groups[1].Value }
  # ③ 标识符
  foreach ($m in [regex]::Matches($label, '[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*')) { if ($m.Value.Length -ge 5) { $raw += $m.Value } }
  $raw = $raw | Where-Object { $_ } | Sort-Object -Property Length -Descending -Unique
  foreach ($needle in $raw) {
    $hits = @()
    for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i].Contains($needle)) { $hits += ($i + 1) } }
    if ($hits.Count -eq 1 -and -not (Is-Bad $lines[$hits[0] - 1])) { return $hits[0] }
  }
  return -1
}

$bt = [char]0x60
$repointed = New-Object System.Collections.Generic.List[string]
$demoted   = New-Object System.Collections.Generic.List[string]

foreach ($doc in $docs) {
  $full = Join-Path $root $doc
  if (-not (Test-Path $full)) { continue }
  $lines = [System.IO.File]::ReadAllLines($full, [System.Text.Encoding]::UTF8)
  $out = New-Object System.Collections.Generic.List[string]
  $ln = 0
  foreach ($line in $lines) {
    $ln++
    $text = $line
    # 从右往左处理，避免改前面影响后面 Index
    $cands = @()
    foreach ($m in $fullRx.Matches($text)) { $cands += [pscustomobject]@{Kind='full'; Idx=$m.Index; Len=$m.Length; File=$m.Groups[1].Value.Trim(); N=$m.Groups[2].Value; End=$m.Index+$m.Length} }
    foreach ($m in $bareRx.Matches($text)) {
      $pre = if ($m.Index -ge 2) { $text.Substring($m.Index - 2, 2) } else { '' }
      if ($pre -eq 'rs' -or $pre -eq 'js') { continue }
      $f = $null
      foreach ($fm in $fileOnLineRx.Matches($text.Substring(0, $m.Index))) { $f = $fm.Groups[1].Value.Trim() }
      if (-not $f) { continue }
      $cands += [pscustomobject]@{Kind='bare'; Idx=$m.Index; Len=$m.Length; File=$f; N=$m.Groups[1].Value; End=$m.Index+$m.Length}
    }
    if ($cands.Count -eq 0) { $out.Add($text); continue }

    foreach ($c in ($cands | Sort-Object -Property Idx -Descending)) {
      $src = Get-Lines $c.File
      if (-not $src) { continue }
      $n = [int]$c.N
      if ($n -ge 1 -and $n -le $src.Count -and -not (Is-Bad $src[$n - 1])) { continue }   # 没坏
      $lab = $null
      $lm = $labelAfterRx.Match($text.Substring($c.End))
      if ($lm.Success) { $lab = $lm.Groups[1].Value.Trim() }
      $target = if ($lab) { Find-Unique $c.File $lab } else { -1 }

      if ($target -gt 0) {
        $oldTxt = if ($c.Kind -eq 'full') { "$bt$($c.File)`:$($c.N)$bt" } else { "$bt`:$($c.N)$bt" }
        $newTxt = if ($c.Kind -eq 'full') { "$bt$($c.File)`:$target$bt" } else { "$bt`:$target$bt" }
        $text = $text.Substring(0, $c.Idx) + $newTxt + $text.Substring($c.End)
        $repointed.Add("$doc`:$ln  $($c.File)`:$($c.N) -> $target   [$lab]")
      } else {
        $oldTxt = if ($c.Kind -eq 'full') { "$bt$($c.File)`:$($c.N)$bt" } else { "$bt`:$($c.N)$bt" }
        $newTxt = if ($c.Kind -eq 'full') { "$bt$($c.File)$bt" } else { '' }
        $text = $text.Substring(0, $c.Idx) + $newTxt + $text.Substring($c.End)
        $demoted.Add("$doc`:$ln  $($c.File)`:$($c.N)  降级为纯文本（无唯一符号）")
      }
    }
    $out.Add($text)
  }
  if ($Apply) { [System.IO.File]::WriteAllLines($full, $out, (New-Object System.Text.UTF8Encoding($false))) }
  Write-Output ("{0,-26} done" -f $doc)
}

Write-Output ''
Write-Output "repointed: $($repointed.Count)   demoted: $($demoted.Count)   mode: $(if($Apply){'APPLY'}else{'dry-run'})"
Write-Output ''
Write-Output '== 重指到符号所在行 =='
foreach ($r in $repointed) { Write-Output "  $r" }
Write-Output ''
Write-Output '== 降级为纯文本（无唯一符号，保留叙述、去掉行号）=='
foreach ($d in $demoted) { Write-Output "  $d" }
