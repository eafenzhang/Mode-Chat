# Verify every cited file:line anchor in docs/parity-matrix.md and docs/prd-0.5.md.
# Checks: file resolves under one of the three roots, line number in range, and (for
# cited lines that name a symbol in backticks) that the line is non-empty.
param([switch]$UpdateAnchors)
# -UpdateAnchors：把当前每个锚点指向的源码行内容重新记入 docs/parity-anchors.lock。
# 只有在「确认锚点已按新代码重新映射过」之后才该跑它；它本身不做任何映射。
$ErrorActionPreference = 'Stop'
$root = 'D:\ModeChat'
$ds   = 'D:\Mode Bot\DS-Bot'
$rpi  = 'C:\Users\Administrator\.cargo\git\checkouts\pi-rust-ba8012a5f17fe953\29d8686'
$roots = @($root, $ds, $rpi)

function Resolve-Path3([string]$p) {
  if ([System.IO.Path]::IsPathRooted($p)) { if (Test-Path $p) { return $p } else { return $null } }
  foreach ($r in $roots) {
    $full = Join-Path $r $p
    if (Test-Path $full) { return $full }
  }
  return $null
}

# Anchors look like `path/file.ext:12` or `path/file.ext:12-34`.
# 锚点有三种写法，全都要覆盖：
#   1) `path:12`  2) `path:12-34`  3) `path:12/34/56`（同一文件的多个位置）
$rx = [regex]'`([A-Za-z0-9_./\\\- ]+\.(?:rs|js|mjs|md|json|yml|toml)):(\d+)(?:-(\d+)|\d+)?((?:/\d+)*)`'
$docs = @('docs/parity-matrix.md', 'docs/prd-0.5.md')
$seen = @{}
$bad = New-Object System.Collections.Generic.List[string]
$checked = 0

foreach ($doc in $docs) {
  $text = [System.IO.File]::ReadAllText((Join-Path $root $doc))
  foreach ($m in $rx.Matches($text)) {
    $rel = $m.Groups[1].Value.Trim()
    # Skip prose fragments that are not paths.
    if ($rel -match '\s' -and $rel -notmatch '/') { continue }
    # 逐个位置校验：`a.rs:1`、`a.rs:1-9`、`a.rs:1/2/3` 里的每个数字都是一处引用。
    # 区间终点（`a.rs:1-9` 的 9）不参与内容校验：区间以 `}` 收尾是正常的。
    $positions = New-Object System.Collections.Generic.List[int]
    $rangeEnd = -1
    $positions.Add([int]$m.Groups[2].Value)
    if ($m.Groups[3].Success) { $rangeEnd = [int]$m.Groups[3].Value; $positions.Add($rangeEnd) }
    foreach ($extra in ($m.Groups[4].Value -split '/' | Where-Object { $_ -ne '' })) {
      $positions.Add([int]$extra)
    }
    $full = Resolve-Path3 $rel
    if (-not $full) { $bad.Add("$rel :$($positions[0]) -> FILE NOT FOUND"); continue }
    $lines = [System.IO.File]::ReadAllLines($full)
    foreach ($start in $positions) {
      $key = "$rel|$start"
      if ($seen.ContainsKey($key)) { continue }
      $seen[$key] = $true
      $checked++
      if ($start -lt 1 -or $start -gt $lines.Count) {
        $bad.Add("$rel :$start -> OUT OF RANGE (max $($lines.Count))"); continue
      }
      if ($start -eq $rangeEnd -and $start -ne $positions[0]) { continue }
      # 结构性空行也算坏锚点：只有括号/分号/逗号的收尾行说明不了任何东西，
      # 指到这里的锚点等于没指（前端重排行号后最常见的就是漂到 `}` 上）。
      $line = $lines[$start - 1].Trim()
      if ($line -eq '') {
        $bad.Add("$rel :$start -> EMPTY LINE")
      } elseif ($line -match '^[\(\)\[\]\{\};,]+$') {
        $bad.Add("$rel :$start -> STRUCTURAL LINE (only brackets/punctuation: $line)")
      }
    }
  }
}

# --- 符号就近校验：`file:N`（`symbol`） 形式必须能在 N±6 行内找到该符号 -------------
# 防止「行号重排后指向另一段空/无关代码」这种静默漂移：只查非空行挡不住它。
$nearRx = [regex]'`([A-Za-z0-9_./\\-]+\.(?:rs|js|mjs)):(\d+)(?:-\d+)?`\s*[（(]\s*`([^`]+)`'
$nearChecked = 0
foreach ($doc in $docs) {
  $text = [System.IO.File]::ReadAllText((Join-Path $root $doc))
  foreach ($m in $nearRx.Matches($text)) {
    $rel = $m.Groups[1].Value.Trim()
    $start = [int]$m.Groups[2].Value
    # 只取字面前缀（截到第一个 < 或空格），模板参数不参与比较
    $sym = ($m.Groups[3].Value -split '[<\s（(]')[0].Trim()
    # 只比对最后一段（`Command::AskBot` -> `AskBot`），并去掉尾随的 ( { :
    $sym = ($sym -split '::')[-1]
    $sym = $sym.TrimEnd('(', '{', ':', ' ') -replace '\s+\$',''
    if ($sym.Length -lt 4) { continue }
    if ($sym -match '[{},\*<>]') { continue }
    $full = Resolve-Path3 $rel
    if (-not $full) { continue }
    $lines = [System.IO.File]::ReadAllLines($full)
    $lo = [Math]::Max(1, $start - 20)
    $hi = [Math]::Min($lines.Count, $start + 20)
    $found = $false
    for ($i = $lo; $i -le $hi; $i++) {
      if ($lines[$i - 1].Contains($sym)) { $found = $true; break }
    }
    $nearChecked++
    if (-not $found) {
      $bad.Add("SYMBOL DRIFT -> $rel :$start 附近无 ``$sym``（±20 行内未命中）")
    }
  }
}
Write-Output "anchors checked: $checked  symbol-checked: $nearChecked"

# --- 裸行号锚点：` :760/767` 这种省略文件名的写法 ---------------------------
# 表格里常写「`src/goals.js:19`（…）、`:760/767`（…）」，后半截只有行号。
# 回填规则：取**同一行内、紧挨在它前面**最近的那个完整锚点的文件名。
# 不跨行继承（会串到上一段/上一列提到的文件上），同一行前面没有文件名就跳过并计数。
$combinedRx = [regex]'`([A-Za-z0-9_./\\-]+\.(?:rs|js|mjs|md|json|yml|toml)):(\d+(?:/\d+)*)(?:-\d+)?`|`:(\d+(?:/\d+)*)(?:-\d+)?`'
$bareChecked = 0
$bareUnresolved = 0
foreach ($doc in $docs) {
  $docPath = Join-Path $root $doc
  foreach ($line in [System.IO.File]::ReadAllLines($docPath)) {
    if ($line -notmatch '`:') { continue }
    $currentFile = $null
    $currentFull = $null
    foreach ($m in $combinedRx.Matches($line)) {
      if ($m.Groups[1].Success) {
        $currentFile = $m.Groups[1].Value.Trim()
        $currentFull = $m.Groups[1].Value.Trim()
        continue
      }
      # 裸行号：归属同一行内紧邻其前的完整锚点文件
      if (-not $currentFile) { $bareUnresolved++; continue }
      $full = Resolve-Path3 $currentFile
      if (-not $full) { $bareUnresolved++; continue }
      $lines = [System.IO.File]::ReadAllLines($full)
      foreach ($n in ($m.Groups[3].Value -split '/' | Where-Object { $_ -ne '' })) {
        $start = [int]$n
        $key = "$currentFile|$start"
        if ($seen.ContainsKey($key)) { continue }
        $seen[$key] = $true
        $bareChecked++
        if ($start -lt 1 -or $start -gt $lines.Count) {
          $bad.Add("$currentFile :$start (裸行号) -> OUT OF RANGE (max $($lines.Count))"); continue
        }
        $code = $lines[$start - 1].Trim()
        if ($code -eq '') {
          $bad.Add("$currentFile :$start (裸行号) -> EMPTY LINE")
        } elseif ($code -match '^[\(\)\[\]\{\};,]+$') {
          $bad.Add("$currentFile :$start (裸行号) -> STRUCTURAL LINE (only brackets/punctuation: $code)")
        }
      }
    }
  }
}
Write-Output "bare-line anchors checked: $bareChecked  skipped(no file earlier on the line): $bareUnresolved"
# --- 锚点指纹锁：没有写符号名的锚点也要防漂移 -------------------------------
# 记录每个 `file:N` 当时指向那一行的内容摘要；下次跑若内容变了，说明代码移动过，
# 该锚点极可能已指错地方（前端重排 main.rs 行号时就是这样把一批锚点指到 `}` 上的）。
$lockPath = Join-Path $root 'docs/parity-anchors.lock'
$lockLines = New-Object System.Collections.Generic.List[string]
$lockChecked = 0
$lockDrift = 0
foreach ($doc in $docs) {
  $text = [System.IO.File]::ReadAllText((Join-Path $root $doc))
  foreach ($m in $rx.Matches($text)) {
    $rel = $m.Groups[1].Value.Trim()
    $positions = New-Object System.Collections.Generic.List[int]
    $positions.Add([int]$m.Groups[2].Value)
    if ($m.Groups[3].Success) { $positions.Add([int]$m.Groups[3].Value) }
    foreach ($extra in ($m.Groups[4].Value -split '/' | Where-Object { $_ -ne '' })) {
      $positions.Add([int]$extra)
    }
    $full = Resolve-Path3 $rel
    if (-not $full) { continue }
    $lines = [System.IO.File]::ReadAllLines($full)
    foreach ($start in $positions) {
      if ($start -lt 1 -or $start -gt $lines.Count) { continue }
      # 指纹：去空白 + 去掉行尾逗号，避免纯格式调整误报
      $sig = ($lines[$start - 1].Trim() -replace '\s+', ' ').TrimEnd(',')
      if ($sig.Length -gt 80) { $sig = $sig.Substring(0, 80) }
      $lockLines.Add("$rel`:$start|$sig")
    }
  }
}
$lockLines = $lockLines | Sort-Object -Unique
if ($UpdateAnchors) {
  [System.IO.File]::WriteAllLines($lockPath, $lockLines, (New-Object System.Text.UTF8Encoding($false)))
  Write-Output "anchor lock rewritten: $($lockLines.Count) entries -> docs/parity-anchors.lock"
} elseif (Test-Path $lockPath) {
  $locked = @{}
  foreach ($l in [System.IO.File]::ReadAllLines($lockPath, [System.Text.Encoding]::UTF8)) {
    $i = $l.IndexOf('|')
    if ($i -gt 0) { $locked[$l.Substring(0, $i)] = $l.Substring($i + 1) }
  }
  foreach ($l in $lockLines) {
    $i = $l.IndexOf('|')
    $key = $l.Substring(0, $i)
    $sig = $l.Substring($i + 1)
    if (-not $locked.ContainsKey($key)) { continue }
    $lockChecked++
    if ($locked[$key] -ne $sig) {
      $lockDrift++
      # 只在明细里提示，前 5 条打出来，避免刷屏
      if ($lockDrift -le 5) {
        $bad.Add("ANCHOR MOVED -> $key 指向内容已变（原 [$($locked[$key])]，现 [$sig]）")
      }
    }
  }
  Write-Output "anchor-fingerprint checked: $lockChecked  drifted: $lockDrift"
} else {
  Write-Output 'anchor-fingerprint: no lock file yet (run with -UpdateAnchors once)'
}
# --- 状态计数一致性：汇总表数字必须等于明细行实际计数 -------------------------
# 明细行形如 `| A01 | ... | ✅ | ... |`；按行首 ID 归组，DOMAIN 表同样核对。
$matrix = [System.IO.File]::ReadAllLines((Join-Path $root 'docs/parity-matrix.md'), [System.Text.Encoding]::UTF8)
$rowRx = [regex]'^\|\s*([A-R])(\d{2})\s*\|'
$domains = @{}
$rowTotal = 0
foreach ($line in $matrix) {
  if ($line -notmatch $rowRx) { continue }
  $rowTotal++
  $domain = $Matches[1]
  if (-not $domains.ContainsKey($domain)) {
    $domains[$domain] = @{ total = 0; ok = 0; half = 0; miss = 0 }
  }
  $d = $domains[$domain]
  $d.total++
  # 只认「状态列」那一段：ID 之后的第一个 ✅/🟡/❌
  if ($line -match '\|\s*(✅|🟡|❌)\s*\|') {
    switch ($Matches[1]) {
      '✅' { $d.ok++ }
      '🟡' { $d.half++ }
      '❌' { $d.miss++ }
    }
  } else {
    $bad.Add("MATRIX ROW MISMATCH -> $domain$($Matches[2]) has no ✅/🟡/❌ status cell")
  }
}
$sumOk = ($domains.Values | ForEach-Object { $_.ok } | Measure-Object -Sum).Sum
$sumHalf = ($domains.Values | ForEach-Object { $_.half } | Measure-Object -Sum).Sum
$sumMiss = ($domains.Values | ForEach-Object { $_.miss } | Measure-Object -Sum).Sum
Write-Output "matrix rows: $rowTotal  actual $(if ($null -eq $sumOk) { 0 } else { $sumOk })/$(if ($null -eq $sumHalf) { 0 } else { $sumHalf })/$(if ($null -eq $sumMiss) { 0 } else { $sumMiss })"

# 汇总表：`| ✅ 已有 | 73 | **91** | 66.9% |` 取第二列之后加粗的 v0.5.0 数
$sumText = ($matrix -join "`n")
function Declared([string]$pattern) {
  $m = [regex]::Match($sumText, $pattern)
  if (-not $m.Success) { return $null }
  return [int]$m.Groups[1].Value
}
$declOk = Declared '\|\s*✅ 已有\s*\|\s*\d+\s*\|\s*\*\*(\d+)\*\*'
$declHalf = Declared '\|\s*🟡 半成品\s*\|\s*\d+\s*\|\s*\*\*(\d+)\*\*'
$declMiss = Declared '\|\s*❌ 缺失\s*\|\s*\d+\s*\|\s*\*\*(\d+)\*\*'
$declTotal = Declared '\|\s*合计\s*\|\s*(\d+)\s*\|'
foreach ($pair in @(
  @{ name = '✅'; declared = $declOk; actual = $sumOk },
  @{ name = '🟡'; declared = $declHalf; actual = $sumHalf },
  @{ name = '❌'; declared = $declMiss; actual = $sumMiss },
  @{ name = '合计'; declared = $declTotal; actual = $rowTotal }
)) {
  if ($null -eq $pair.declared) {
    $bad.Add("SUMMARY TABLE -> missing row for $($pair.name)")
  } elseif ([int]$pair.declared -ne [int]$pair.actual) {
    $bad.Add("SUMMARY DRIFT -> $($pair.name) 汇总表写 $($pair.declared)，明细实际 $($pair.actual)")
  }
}

# 分域明细表：`| A Bot 身份 | 13 | 9/4/0 | ... |`
$domRx = [regex]'\|\s*([A-R])\s+[^|]*\|\s*(\d+)\s*\|\s*(\d+)/(\d+)/(\d+)\s*\|'
foreach ($m in $domRx.Matches($sumText)) {
  $d = $m.Groups[1].Value
  if (-not $domains.ContainsKey($d)) { $bad.Add("DOMAIN TABLE -> $d has no detail rows"); continue }
  $a = $domains[$d]
  $decl = "$($m.Groups[2].Value) $($m.Groups[3].Value)/$($m.Groups[4].Value)/$($m.Groups[5].Value)"
  $act = "$($a.total) $($a.ok)/$($a.half)/$($a.miss)"
  if ($decl -ne $act) {
    $bad.Add("DOMAIN DRIFT -> $d 分域表写 [$decl]，明细实际 [$act]")
  }
}

# 每条非 ✅ 行必须有降级说明
foreach ($line in $matrix) {
  if ($line -notmatch $rowRx) { continue }
  if ($line -match '\|\s*✅\s*\|') { continue }
  if ($line -notmatch '〔降级') {
    $bad.Add("NO DOWNGRADE NOTE -> $($Matches[1])$($Matches[2]) 既不是 ✅ 也没有〔降级 …〕说明")
  }
}

if ($bad.Count -eq 0) {
  Write-Output 'RESULT: all anchors resolve, summary counts consistent'
} else {
  Write-Output "RESULT: $($bad.Count) problem(s)"
  $bad | ForEach-Object { Write-Output "  $_" }
  exit 1
}
