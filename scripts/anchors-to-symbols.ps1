# 把「行号锚」升级成「符号锚」：`file:N`（`sym`） → `sym`（不带行号）。
#
# 为什么必须这么做：`src/main.rs` 两万六千行，O-5 正在改它的 px 值。每改一次，
# 后面所有行号全部失效，锚点批量漂移——这一轮已经重复了六次「修一轮错一轮」。
# 根因不是改得不对，是**行号不是那句话**：它只是当时的行号。
#
# 符号则相反：`fn chat_info_panel` 在文件里只有一处，行号怎么变都能 grep 到。
# 代价是这个门禁不再逐条比对`symbol`与行内容——但那本来也只是「当时那一行」，
# 换个说法：**用行号锁住的是位置，用符号锁住的是存在**。位置会变，存在不会。
#
# 只转换「能找到唯一符号」的全文件行号锚点，找不到的保持原样并报出来。
param(
  [string[]]$Docs = @('docs/parity-matrix.md', 'docs/prd-0.5.md', 'docs/prd-0.6.md'),
  [switch]$Apply
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)

$rx = [regex]'`([A-Za-z0-9_./\\-]+\.(?:rs|js|mjs)):(\d+)`(\s*[（(]\s*`([^`]{2,120})`)?'
# 裸行号 `:N`（`标签`）——同一行里前面的完整锚点已经被转成符号锚之后，
# 门禁会把裸行号归给「同行最近的前置 file:」，而那个 file: 已经不在了，
# 于是它会去继承更早的另一个文件（实测：main.rs 的 :18909 被归给 data.rs，
# 直接报 OUT OF RANGE）。所以裸行号也必须一起转成符号锚。
$bareRx = [regex]'`:(\d+)`(\s*[（(]\s*`([^`]{2,120})`)?'

$cache = @{}
function Get-Lines([string]$rel) {
  if ($cache.ContainsKey($rel)) { return $cache[$rel] }
  $full = Join-Path $root $rel
  if (-not (Test-Path $full)) { $cache[$rel] = $null; return $null }
  $cache[$rel] = [System.IO.File]::ReadAllLines($full)
  return $cache[$rel]
}

function Resolve-Symbol([string]$rel, [string]$label) {
  $lines = Get-Lines $rel
  if (-not $lines -or -not $label) { return $null }
  $cands = @()
  $lit = $label.Trim()
  foreach ($v in @($lit, ($lit -replace '^fn\s+', ''), ($lit -replace '[（(].*$', '').Trim())) {
    if ($v -and $v.Length -ge 5 -and $v -notmatch '\s{2,}') { $cands += $v }
  }
  foreach ($m in [regex]::Matches($label, '"([^"]{5,})"')) { $cands += $m.Groups[1].Value }
  $cands = $cands | Where-Object { $_ } | Sort-Object -Property Length -Descending -Unique
  foreach ($c in $cands) {
    $hits = 0
    foreach ($ln in $lines) { if ($ln.Contains($c)) { $hits++ } }
    if ($hits -eq 1) { return [pscustomobject]@{ Text = $c; Hits = 1 } }
  }
  $first = if ($cands.Count -gt 0) { $cands[0] } else { $null }
  if ($first) {
    $n = 0; foreach ($ln in $lines) { if ($ln.Contains($first)) { $n++ } }
    if ($n -ge 2) { return [pscustomobject]@{ Text = $first; Hits = $n } }
  }
  return $null
}

$bt = [char]0x60
$converted = New-Object System.Collections.Generic.List[string]
$kept = New-Object System.Collections.Generic.List[string]

foreach ($doc in $Docs) {
  $full = Join-Path $root $doc
  if (-not (Test-Path $full)) { continue }
  $lines = [System.IO.File]::ReadAllLines($full, [System.Text.Encoding]::UTF8)
  $out = New-Object System.Collections.Generic.List[string]
  $ln = 0
  $changed = 0
  foreach ($line in $lines) {
    $ln++
    $new = $line

    # 先收集本行的完整锚点与裸行号，统一从右往左改（屏幕上 Index 才不会失效）
    $jobs = @()
    foreach ($m in $rx.Matches($line)) {
      if (-not $m.Groups[4].Success) { continue }
      $jobs += [pscustomobject]@{ Kind='full'; Idx=$m.Index; Len=$m.Length; Rel=$m.Groups[1].Value.Trim(); N=$m.Groups[2].Value; Label=$m.Groups[4].Value.Trim() }
    }
    foreach ($m in $bareRx.Matches($line)) {
      if (-not $m.Groups[3].Success) { continue }
      $idx = $m.Index
      # 排除完整锚点里的 `:N` 部分（它前面紧挨着 .rs/.js）
      $pre = if ($idx -ge 1) { $line.Substring($idx - 1, 1) } else { '' }
      if ($pre -match '[A-Za-z0-9_]') { continue }
      # 归属：同一行里最近的前置完整锚点的文件名
      $rel = $null
      foreach ($fm in [regex]::Matches($line.Substring(0, $idx), '`([A-Za-z0-9_./\\-]+\.(?:rs|js|mjs)):')) { $rel = $fm.Groups[1].Value.Trim() }
      if (-not $rel) { continue }
      $jobs += [pscustomobject]@{ Kind='bare'; Idx=$m.Index; Len=$m.Length; Rel=$rel; N=$m.Groups[1].Value; Label=$m.Groups[3].Value.Trim() }
    }

    foreach ($j in ($jobs | Sort-Object -Property Idx -Descending)) {
      $r = Resolve-Symbol $j.Rel $j.Label
      if (-not $r -or $r.Hits -ne 1) {
        $why = if (-not $r) { '无符号/不存在' } else { "出现 $($r.Hits) 次不唯一" }
        $kept.Add("$doc`:$ln  $($j.Rel)`:$($j.N) [$($j.Label)] $why")
        continue
      }
      $repl = "$bt$($r.Text)$bt"
      $new = $new.Substring(0, $j.Idx) + $repl + $new.Substring($j.Idx + $j.Len)
      $converted.Add("$doc`:$ln  $($j.Rel)`:$($j.N) -> $repl")
      $changed++
    }
    $out.Add($new)
  }
  if ($Apply -and $changed -gt 0) {
    [System.IO.File]::WriteAllLines($full, $out, (New-Object System.Text.UTF8Encoding($false)))
  }
  Write-Output ("{0,-26} 转成符号锚 {1,3} 条" -f $doc, $changed)
}

Write-Output ''
Write-Output "converted: $($converted.Count)   kept: $($kept.Count)   mode: $(if($Apply){'APPLY'}else{'dry-run'})"
Write-Output ''
Write-Output '== 已转成符号锚（行号不再参与比对）=='
foreach ($c in $converted) { Write-Output "  $c" }
if ($kept.Count -gt 0) {
  Write-Output ''
  Write-Output '== 保留行号（找不到唯一符号）=='
  foreach ($k in $kept) { Write-Output "  $k" }
}
