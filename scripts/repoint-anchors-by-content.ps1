# 按「内容」重指锚点：用锁里存的旧指纹反查这一行搬到哪里去了。
#
# 为什么需要它：`-UpdateAnchors` 只按「文档里现在写的行号」重新取内容当指纹，
# 代码位移后它会心平气和地把错位的内容固化成新基线。人工按符号重指又要求
# 「知道这句原来指什么」，而代码一动，人算出来的行号几分钟就过期。
#
# 这个脚本换一个来源：锁里存着「那条锚点当时指向的那一行内容」，内容是不动的。
# 拿旧指纹到文件里找它现在在哪，唯一命中才重指；命中多次或找不到都拒绝并报出来。
#
# 只处理「文档里真实存在的锚点」，锁里那些 stale（文档已删）条目一概不碰。
#
# 三种结果，只有第一种会被改写：
#   MOVED      指纹在文件里恰好出现 1 次 → 给出新行号
#   AMBIGUOUS  出现多次 → 拒绝。这就是「重复行没有区分力」，猜哪个都是错
#   LOST       一次都没出现 → 那行被改过或删了，只能人来判断
#
# 用法（PowerShell 5.1；本文件必须存 UTF-8 BOM，否则中文按 GBK 解会把脚本拆坏）：
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\repoint-anchors-by-content.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\repoint-anchors-by-content.ps1 -Apply
param(
  [switch]$Apply,
  [string[]]$Docs = @('docs/parity-matrix.md', 'docs/prd-0.5.md', 'docs/prd-0.6.md')
)
$ErrorActionPreference = 'Stop'

$root  = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$lockP = Join-Path $root 'docs/parity-anchors.lock'
if (-not (Test-Path $lockP)) { Write-Output 'no lock file'; exit 2 }

# 归一化必须和 verify-parity-anchors.ps1 的 Add-LockEntry 完全一致，
# 否则「同一行」会被算成不同内容，重指就全错。
function Get-Sig([string]$line) {
  $s = ($line.Trim() -replace '\s+', ' ').TrimEnd(',')
  if ($s.Length -gt 80) { $s = $s.Substring(0, 80) }
  return $s
}

$locked = @{}
foreach ($l in [System.IO.File]::ReadAllLines($lockP, [System.Text.Encoding]::UTF8)) {
  $i = $l.IndexOf('|')
  if ($i -gt 0) { $locked[$l.Substring(0, $i)] = $l.Substring($i + 1) }
}

$fileLines = @{}
$fileIndex = @{}
function Get-Lines([string]$rel) {
  if ($fileLines.ContainsKey($rel)) { return $fileLines[$rel] }
  $full = Join-Path $root $rel
  if (-not (Test-Path $full)) { $fileLines[$rel] = $null; return $null }
  $lines = [System.IO.File]::ReadAllLines($full)
  $fileLines[$rel] = $lines
  $idx = @{}
  for ($i = 0; $i -lt $lines.Count; $i++) {
    $sig = Get-Sig $lines[$i]
    if ($sig -eq '') { continue }
    if ($idx.ContainsKey($sig)) { $idx[$sig] += ($i + 1) } else { $idx[$sig] = @($i + 1) }
  }
  $fileIndex[$rel] = $idx
  return $lines
}

# ---- 第一步：扫文档，收集「真实在用的锚点」 --------------------------
$fullRx = [regex]'`([A-Za-z0-9_./\\\- ]+\.(?:rs|js|mjs|md|json|yml|toml)):((?:\d+(?:[/-]))*\d+)`'
$bareRx = [regex]'`:((?:\d+(?:[/-]))*\d+)`'
$fileRx = [regex]'`([A-Za-z0-9_./\\-]+\.(?:rs|js|mjs|md|json|yml|toml)):'

# docAnchors: 每项 = 文档名 / 该行行号 / 文件 / 位置串
$docAnchors = New-Object System.Collections.Generic.List[object]
foreach ($doc in $Docs) {
  $full = Join-Path $root $doc
  if (-not (Test-Path $full)) { continue }
  $docLineNo = 0
  foreach ($line in [System.IO.File]::ReadAllLines($full, [System.Text.Encoding]::UTF8)) {
    $docLineNo++
    $owner = @{}
    foreach ($m in $fileRx.Matches($line)) { $owner[$m.Index] = $m.Groups[1].Value.Trim() }
    foreach ($m in $fullRx.Matches($line)) {
      $docAnchors.Add([pscustomobject]@{ Doc=$doc; DocLine=$docLineNo; File=$m.Groups[1].Value.Trim(); Nums=$m.Groups[2].Value; Index=$m.Index })
    }
    foreach ($m in $bareRx.Matches($line)) {
      $file = $null
      foreach ($k in ($owner.Keys | Sort-Object)) { if ($k -lt $m.Index) { $file = $owner[$k] } }
      if (-not $file) { continue }
      # 已经被完整锚点覆盖的位置不重复计
      $overlap = $false
      foreach ($a in $docAnchors) { if ($a.Doc -eq $doc -and $a.DocLine -eq $docLineNo -and $a.Index -eq $m.Index) { $overlap = $true } }
      if ($overlap) { continue }
      $docAnchors.Add([pscustomobject]@{ Doc=$doc; DocLine=$docLineNo; File=$file; Nums=$m.Groups[1].Value; Index=$m.Index })
    }
  }
}

# ---- 第二步：对每个位置判断漂没漂、漂到哪 ----------------------------
$remap     = @{}
$moved     = New-Object System.Collections.Generic.List[string]
$ambiguous = New-Object System.Collections.Generic.List[string]
$lost      = New-Object System.Collections.Generic.List[string]
$noLock    = 0
$okCount   = 0

foreach ($a in $docAnchors) {
  foreach ($n in ($a.Nums -split '[/-]' | Where-Object { $_ -match '^\d+$' })) {
    $key = "$($a.File):$n"
    if (-not $locked.ContainsKey($key)) { $noLock++; continue }
    $lines = Get-Lines $a.File
    if (-not $lines) { continue }
    $sig = $locked[$key]
    if ($sig -eq '') { continue }
    if ([int]$n -le $lines.Count -and (Get-Sig $lines[[int]$n - 1]) -eq $sig) { $okCount++; continue }
    $hits = $fileIndex[$a.File][$sig]
    if ($null -eq $hits) {
      $lost.Add("$key  [$sig]")
    } elseif ($hits.Count -eq 1) {
      $remap["$($a.File)|$n"] = $hits[0]
      $moved.Add("$key -> $($hits[0])  [$sig]")
    } else {
      $show = ($hits | Select-Object -First 8) -join ','
      $more = if ($hits.Count -gt 8) { " …共 $($hits.Count) 处" } else { '' }
      $ambiguous.Add("$key  命中 $($hits.Count) 处（$show$more）  [$sig]")
    }
  }
}

Write-Output "文档锚点数: $($docAnchors.Count)   未在锁内: $noLock   未漂: $okCount   MOVED: $($moved.Count)   AMBIGUOUS: $($ambiguous.Count)   LOST: $($lost.Count)"
Write-Output ''

function Remap-Line([string]$line, [ref]$hits) {
  $owner = @{}
  foreach ($m in $fileRx.Matches($line)) { $owner[$m.Index] = $m.Groups[1].Value.Trim() }
  $tokens = @()
  foreach ($m in $fullRx.Matches($line)) { $tokens += [pscustomobject]@{Index=$m.Index;Len=$m.Length;Text=$m.Value;File=$m.Groups[1].Value.Trim();Nums=$m.Groups[2].Value} }
  foreach ($m in $bareRx.Matches($line)) {
    $file = $null
    foreach ($k in ($owner.Keys | Sort-Object)) { if ($k -lt $m.Index) { $file = $owner[$k] } }
    if (-not $file) { continue }
    $dup = $false
    foreach ($t in $tokens) { if ($t.Index -eq $m.Index) { $dup = $true } }
    if ($dup) { continue }
    $tokens += [pscustomobject]@{Index=$m.Index;Len=$m.Length;Text=$m.Value;File=$file;Nums=$m.Groups[1].Value}
  }
  if ($tokens.Count -eq 0) { return $line }

  $out = $line
  foreach ($t in ($tokens | Sort-Object -Property Index -Descending)) {
    $newNums = ($t.Nums -split '([/-])' | ForEach-Object {
      if ($_ -match '^\d+$' -and $remap.ContainsKey("$($t.File)|$_")) { $hits.Value++; "$($remap["$($t.File)|$_"])" } else { $_ }
    }) -join ''
    if ($newNums -ne $t.Nums) {
      $out = $out.Substring(0, $t.Index) + $t.Text.Replace($t.Nums, $newNums) + $out.Substring($t.Index + $t.Len)
    }
  }
  return $out
}

$totalHits = 0
foreach ($doc in $Docs) {
  $full = Join-Path $root $doc
  if (-not (Test-Path $full)) { continue }
  $lines = [System.IO.File]::ReadAllLines($full, [System.Text.Encoding]::UTF8)
  $changed = 0; $hits = 0
  $new = New-Object System.Collections.Generic.List[string]
  foreach ($line in $lines) {
    $h = 0
    $mapped = Remap-Line $line ([ref]$h)
    $hits += $h
    if ($mapped -ne $line) { $changed++ }
    $new.Add($mapped)
  }
  if ($Apply -and $changed -gt 0) {
    [System.IO.File]::WriteAllLines($full, $new, (New-Object System.Text.UTF8Encoding($false)))
  }
  $totalHits += $hits
  Write-Output ("{0,-26} 改写行数 {1,3}  改写锚点 {2,3}" -f $doc, $changed, $hits)
}

Write-Output ''
Write-Output "anchors rewritten: $totalHits   mode: $(if($Apply){'APPLY'}else{'dry-run（加 -Apply 才写文件）'})"

if ($ambiguous.Count -gt 0) {
  Write-Output ''
  Write-Output '== AMBIGUOUS（内容出现多次，拒绝自动重指；要人工按符号定位）=='
  foreach ($a in $ambiguous) { Write-Output "  $a" }
}
if ($lost.Count -gt 0) {
  Write-Output ''
  Write-Output '== LOST（旧指纹在文件里已不存在，那行被改过或删了）=='
  foreach ($a in $lost) { Write-Output "  $a" }
}
if (-not $Apply) { Write-Output ''; Write-Output 'dry-run：以上只是候选，未写文件。' }
