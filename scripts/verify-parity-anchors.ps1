# Verify every cited file:line anchor in docs/parity-matrix.md, docs/prd-0.5.md and docs/prd-0.6.md.
# Checks: file resolves under one of the three roots, line number in range, and (for
# cited lines that name a symbol in backticks) that the line is non-empty.
param(
  [switch]$UpdateAnchors,
  [ValidateSet('Head', 'WorkTree')][string]$Baseline = 'Head'
)
# -UpdateAnchors：把当前每个锚点指向的源码行内容重新记入 docs/parity-anchors.lock。
# 只有在「确认锚点已按新代码重新映射过」之后才该跑它；它本身不做任何映射。
$ErrorActionPreference = 'Stop'
# 仓库根从脚本位置反推（脚本在 scripts/ 下），这样 CI 的临时检出目录也能跑；
# 两个外部对照树（DS-Bot 基线、pi-rust checkout）可选，用
# MODE_PARITY_DS_ROOT / MODE_PARITY_RPI_ROOT 指过去，没有就跳过（不当作 OK）。
$root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$ds   = if ($env:MODE_PARITY_DS_ROOT)  { $env:MODE_PARITY_DS_ROOT }  else { '' }
$rpi  = if ($env:MODE_PARITY_RPI_ROOT) { $env:MODE_PARITY_RPI_ROOT } else { '' }
$roots = @($root)
if ($ds)  { $roots += $ds }
if ($rpi) { $roots += $rpi }

# 外部对照树里的路径（DS-Bot 基线、pi-rust checkout）：CI 里没有这些树，
# 找不到时跳过并计数，不当成坏锚点；但仓库自己的路径找不到必须报错。
function External-Kind([string]$rel) {
  if ($rel -like 'ds-bot/*' -or $rel -eq 'README.zh.md' -or $rel -eq 'README.md') { return 'ds' }
  if ($rel -like 'rpi-*/*' -or $rel -like 'crates/rpi-harness/*') { return 'rpi' }
  return $null
}
$script:skippedExternal = 0

function Resolve-Path3([string]$p) {
  if ([System.IO.Path]::IsPathRooted($p)) { if (Test-Path $p) { return $p } else { return $null } }
  foreach ($r in $rootOrder) {
    $full = Join-Path $r $p
    if (Test-Path $full) { return $full }
  }
  return $null
}

# --- 基线：默认按 `git HEAD` 的已提交内容校验，而不是工作区 ---------------------
# 锚点是对着「已提交的代码」写的。工作区若有人正在改 src/main.rs，行号会整体偏移，
# 让好锚点看起来像指到了 `}`（本轮前端 +45 行就制造了 10 个假失败）。
# 用 `-Baseline WorkTree` 可以改回校验工作区（改完代码、准备重排锚点前用）。
if (-not $Baseline) { $Baseline = 'Head' }
$baselineDir = $null
if ($Baseline -eq 'Head') {
  $baselineDir = Join-Path ([System.IO.Path]::GetTempPath()) ('parity-head-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
  New-Item -ItemType Directory -Path $baselineDir -Force | Out-Null
  # 只导出锚点会用到的源码树，别把整个仓库拷出来。
  foreach ($sub in @('src', 'crates')) {
    $paths = & git -C $root ls-tree -r --name-only HEAD -- $sub 2>$null
    foreach ($rel in $paths) {
      if ($rel -notmatch '\.(rs|js|mjs|toml)$') { continue }
      $dst = Join-Path $baselineDir $rel
      $dir = Split-Path -Parent $dst
      if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
      & git -C $root show "HEAD:$rel" 2>$null | Set-Content -LiteralPath $dst -Encoding UTF8
    }
  }
  # 文档也必须一起取 HEAD 版本，否则「-Baseline Head」只对了一半：
  # 源码是 HEAD 的，矩阵/PRD/锁却是工作区的。别人正在改 docs/ 时，同一 HEAD
  # 连跑三次会给出三种结果（实测 5 problem / 5 problem / 3 problem），
  # 这种门禁比没有门禁更糟——你没法判断红的是代码还是别人未提交的文档。
  foreach ($rel in @('docs/parity-matrix.md', 'docs/prd-0.5.md', 'docs/prd-0.6.md', 'docs/parity-anchors.lock')) {
    $blob = & git -C $root show "HEAD:$rel" 2>$null
    if (-not $blob) { continue }
    $dst = Join-Path $baselineDir $rel
    $dir = Split-Path -Parent $dst
    if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
    $blob | Set-Content -LiteralPath $dst -Encoding UTF8
  }
  Write-Output "baseline: git HEAD ($baselineDir)"
} else {
  Write-Output 'baseline: working tree'
}
# 这个临时基线目录每次运行都会导出 src/ + crates/（约 3.5MB），跑完必须删。
# 原来只在开头 New-Item、从不删：本机攒了 137 个目录 / 475MB 才发现。
# 因为脚本末尾有 `exit 1` 分支，用 try/finally 包住整段不现实，
# 改成在**两个出口**都调一次（正常结束、以及 exit 1 之前），并加一个退出钩子兜底。
function Remove-BaselineDir {
  if ($script:baselineDir -and (Test-Path $script:baselineDir)) {
    Remove-Item -LiteralPath $script:baselineDir -Recurse -Force -ErrorAction SilentlyContinue
  }
  $script:baselineDir = $null
}
$rootOrder = if ($baselineDir) { @($baselineDir) + $roots } else { $roots }

# 文档/锁的读取必须和源码走同一套基线解析。
# 写死 `Join-Path $root $doc` 会让 `-Baseline Head` 只对一半：源码 HEAD、文档工作区。
function Resolve-DocPath([string]$rel) {
  foreach ($r in $rootOrder) {
    $full = Join-Path $r $rel
    if (Test-Path $full) { return $full }
  }
  return (Join-Path $root $rel)
}

# Anchors look like `path/file.ext:12` or `path/file.ext:12-34`.
# 锚点有三种写法，全都要覆盖：
#   1) `path:12`  2) `path:12-34`  3) `path:12/34/56`（同一文件的多个位置）
$rx = [regex]'`([A-Za-z0-9_./\\\- ]+\.(?:rs|js|mjs|md|json|yml|toml)):(\d+)(?:-(\d+)|\d+)?((?:/\d+)*)`'
$docs = @('docs/parity-matrix.md', 'docs/prd-0.5.md', 'docs/prd-0.6.md')
$seen = @{}
$bad = New-Object System.Collections.Generic.List[string]
$checked = 0

foreach ($doc in $docs) {
  $text = [System.IO.File]::ReadAllText((Resolve-DocPath $doc))
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
    if (-not $full) {
      $kind = External-Kind $rel
      if ($kind) { $script:skippedExternal++; continue }
      $bad.Add("$rel :$($positions[0]) -> FILE NOT FOUND"); continue
    }
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
  $text = [System.IO.File]::ReadAllText((Resolve-DocPath $doc))
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
Write-Output "anchors checked: $checked  symbol-checked: $nearChecked  skipped-external: $script:skippedExternal"

# --- 反引号没配对的锚点：语法坏了，等于这个锚点完全没被校验 -------------------
# 实测撞到 3 处（`表单 :2479``、`、:16702``、`、:7348``）：前半截该包住行号的反引号丢了，
# 只留了结尾那一个。这种写法两条正则都匹配不到——
#   完整锚点正则要求「`path:N`」整段被反引号包住；
#   裸行号正则要求 `:N` 前面紧跟反引号。
# 于是这几处**既不在位置校验里，也不在指纹锁里**，把行号改成 99999 都照样 PARITY=0。
# 判据：`:N` 前面出现的反引号个数为偶数 -> 它落在反引号对之外 -> 语法坏了。
# 只在本行已有合法反引号锚点时才报，避免把说明文字里的冒号数字当成锚点。
$ideRx = [regex]':(\d+(?:/\d+)*)'
$fileRefRx = [regex]'`[A-Za-z0-9_./\\-]+\.(?:rs|js|mjs|md|json|yml|toml):\d'
$malformed = 0
foreach ($doc in $docs) {
  $docPath = Resolve-DocPath $doc
  if (-not (Test-Path $docPath)) { continue }
  $ln = 0
  foreach ($line in [System.IO.File]::ReadAllLines($docPath, [System.Text.Encoding]::UTF8)) {
    $ln++
    if ($line.Trim() -eq '```' -or -not $fileRefRx.IsMatch($line)) { continue }
    foreach ($m in $ideRx.Matches($line)) {
      $pre = 0
      $upto = $line.Substring(0, $m.Index)
      foreach ($c in $upto.ToCharArray()) { if ($c -eq [char]0x60) { $pre++ } }
      if ($pre % 2 -eq 1) { continue }  # 在反引号对内，正常
      $malformed++
      $bad.Add("MALFORMED ANCHOR -> $doc :$ln  锚点 ``$($m.Value)`` 没被反引号包住（反引号少了一半），位置校验和指纹锁都看不到它；补上开头的反引号即可")
    }
  }
}
if ($malformed -gt 0) { Write-Output "malformed anchors (outside backticks, unchecked): $malformed" }

# --- 裸行号锚点：` :760/767` 这种省略文件名的写法 ---------------------------
# 表格里常写「`src/goals.js:19`（…）、`:760/767`（…）」，后半截只有行号。
# 回填规则：取**同一行内、紧挨在它前面**最近的那个完整锚点的文件名。
# 不跨行继承（会串到上一段/上一列提到的文件上），同一行前面没有文件名就跳过并计数。
$combinedRx = [regex]'`([A-Za-z0-9_./\\-]+\.(?:rs|js|mjs|md|json|yml|toml)):(\d+(?:/\d+)*)(?:-\d+)?`|`:(\d+(?:/\d+)*)(?:-\d+)?`'
$bareChecked = 0
$bareAmbiguous = 0
$bareUnresolved = 0
foreach ($doc in $docs) {
  $docPath = Resolve-DocPath $doc
  foreach ($line in [System.IO.File]::ReadAllLines($docPath)) {
    if ($line -notmatch '`:') { continue }
    $currentFile = $null
    $currentFull = $null
    # 本行在当前位置之前出现过几个**不同**的文件。>1 说明这行的裸行号归属
    # 是「取最近的」而不是「取语义上对的」，可能被插在中间的引用带跑：
    # prd-0.6.md 的 O-3 行就这样把 :25011/:25017 归给了 tool_render.rs，
    # 这次因为越界才报错；行数够长就会静默指到无关代码。只计数、不判失败
    # （文档写法本身没错，错在归属靠猜），把数字摆出来让它别再是隐形的。
    $filesBefore = New-Object System.Collections.Generic.List[string]
    foreach ($m in $combinedRx.Matches($line)) {
      if ($m.Groups[1].Success) {
        $currentFile = $m.Groups[1].Value.Trim()
        $currentFull = $m.Groups[1].Value.Trim()
        if (-not $filesBefore.Contains($currentFile)) { $filesBefore.Add($currentFile) }
        continue
      }
      # 裸行号：归属同一行内紧邻其前的完整锚点文件
      if (-not $currentFile) { $bareUnresolved++; continue }
      if ($filesBefore.Count -gt 1) {
        $bareAmbiguous++
        if ($bareAmbiguous -le 10) {
          Write-Output "  [ambiguous] $doc :$($m.Groups[3].Value) 归给 $currentFile（本行另有 $((($filesBefore | Where-Object { $_ -ne $currentFile }) -join ', '))），语义归属需人工核"
        }
      }
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
Write-Output "bare-line anchors checked: $bareChecked  skipped(no file earlier on the line): $bareUnresolved  ambiguous-file: $bareAmbiguous"

# --- 邻近符号校验：锚点后面的 `（…）` 标签里写的符号必须真的在附近 ---------------
# 挡的是「指到一段无关但合法的代码」——这类漂移行非空、也不是纯括号，前两道检查都放过。
# 实测教训（v0.5.1 审查）：F06 的 `:19273` 实际指向 `.pointer()`、`:7349` 指向一段
# 无关代码，全都「非空、非结构行」地通过了。所以标签里的符号必须在附近出现。
#
# 双阈值，因为锚点的写法本身分两类：
#   - 严格（≤ $STRICT_LINES 行）：锚点就指在符号那一行附近，必须命中；
#   - 宽松（≤ $LOOSE_LINES 行）：锚点指「整块注册表/函数」的起点，符号在块内几十行处
#     （如 `bot_tools.rs` 的 `tools` 里 whoami/list_bots/... 各在 69/78/87/104 行）
#     是合法写法，但会单独计数并在 `-Verbose` 明细里列出，避免整类锚点被迫改成散点。
# 超过宽松阈值的直接判错，并打出每个候选符号的实际最近距离。
$STRICT_LINES = 12
$LOOSE_LINES = 60

$identRx = [regex]'[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*'
$backtickRx = [regex]'[\u0060]([^\u0060]+)[\u0060]'
$lineCache = @{}
function Get-SourceLines([string]$path) {
  if (-not $lineCache.ContainsKey($path)) { $lineCache[$path] = [System.IO.File]::ReadAllLines($path) }
  return ,$lineCache[$path]
}
# 返回 @{ distance = 最近距离（-1 表示标签里没有可校验的标识符）; name = 命中的符号 }
function Measure-Label([string]$relAnchor, [int]$n, [string[]]$candidateIdentifiers) {
  $full = Resolve-Path3 $relAnchor
  if (-not $full) { return $null }
  $lines = Get-SourceLines $full
  if ($n -lt 1 -or $n -gt $lines.Count) { return $null }
  # 候选标识符已经由 Get-Identifiers 筛过一遍（那里负责「是不是符号」的判定）。
  # 这里**不能**再按命名风格过滤一次：那样会把反引号里的 `precheck` / `rows` / `read`
  # 这类全小写短名在第二道口子重新丢掉，改动等于没生效（实测撞过）。
  $idents = @($candidateIdentifiers | Where-Object { $_ -and $_.Length -ge 2 } | Sort-Object -Unique)
  if ($idents.Count -eq 0) { return @{ distance = -1; name = '' } }
  $best = -1
  $bestName = ''
  foreach ($w in $idents) {
    # 只比对最后一段（`RpiSession::compact` -> `compact`）
    $tail = ($w -split '::')[-1]
    for ($i = 0; $i -lt $lines.Count; $i++) {
      if (-not $lines[$i].Contains($tail)) { continue }
      $d = [Math]::Abs(($i + 1) - $n)
      if ($best -lt 0 -or $d -lt $best) { $best = $d; $bestName = $w }
    }
  }
  return @{ distance = $best; name = $bestName }
}

function Get-Identifiers([string]$text) {
  $out = @()
  foreach ($im in $identRx.Matches($text)) {
    $w = $im.Value
    if ($w.Length -lt 4) { continue }
    if ($w -notmatch '_' -and $w -notmatch '::' -and $w -cnotmatch '[A-Z]') { continue }
    $out += $w
  }
  # 反引号里的裸符号一律认作「这是代码符号」，不再追问命名风格。
  # 命名风格（含 _ / :: / 大写）只是「这词是不是符号」的代理判据，反引号是作者显式
  # 声明的符号，信号更强。Rust 里 `read` / `precheck` / `rows` 这类全小写短名最常见，
  # 被风格判据整类否掉是误伤：实测 privacy.rs:201 加不加反引号都被跳过，只有写成
  # privacy::precheck 才过，等于逼文档作者去查模块路径，而这个信息锚点本身不需要。
  # 只认「裸符号」：含 / \ . 或冒号的是路径、锚点或别的引用，不能当本行符号用。
  foreach ($bm in $backtickRx.Matches($text)) {
    $core = $bm.Groups[1].Value.Trim()
    $core = $core -replace '::', '__SEP__'
    if ($core -match '/' -or $core -match '\\' -or $core -match '\.' -or $core -match ':') { continue }
    $core = $core -replace '__SEP__', '::'
    if ($core -notmatch '^[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*$') { continue }
    if ($core.Length -lt 4) { continue }
    $out += $core
  }
  return @($out | Sort-Object -Unique)
}

$labelStrict = 0
$labelLoose = 0
$labelSkipped = 0
$labelMiss = 0
$looseList = New-Object System.Collections.Generic.List[string]
$skipList = New-Object System.Collections.Generic.List[string]

# 单条锚点的符号校验。
# 先用自己的标签；标签里抽不出标识符时（「classic 两处调用」这种纯中文描述），
# 退回**同一表格行**里其它锚点出现过的一整套标识符——同一行讲的是同一个功能，
# 用这一行的符号集合来验，比直接跳过强得多。
# 负向用例就是这么漏的：F06 的 `:19395`（classic 两处调用）改成 `:1200`，
# 因为标签无标识符被判 skipped，而 :1200 那行非空非结构，于是一路放行。
function Test-AnchorLabel([string]$rel, [int]$n, [string]$label, [string]$rowText, [string]$where) {
  $own = @()
  if ($label) { $own = Get-Identifiers $label }
  $source = 'label'
  # 只用锚点自己的标签。曾经试过「标签没符号就退回整行的符号集合」，实测不可行：
  # 矩阵一行有十几个锚点，整行的标识符跟某一个锚点无关，直接制造 11 条误报
  #（`bot_icon` 371 行外、`private_bot_id` 1350 行外…）。标签里没符号的锚点
  # 交给指纹锁兜底（见下方「锁覆盖完整性」），那才是真能挡住漂移的一层。
  if ($own.Count -eq 0) {
    $script:labelSkipped++
    $script:skipList.Add(("{0} :{1}" -f $rel, $n))
    return
  }
  $r = Measure-Label $rel $n $own
  if ($null -eq $r) { return }
  if ($r.distance -lt 0) {
    $script:labelSkipped++
    $script:skipList.Add(("{0} :{1}" -f $rel, $n))
    return
  }
  if ($r.distance -le $STRICT_LINES) {
    $script:labelStrict++
    return
  }
  if ($r.distance -le $LOOSE_LINES) {
    $script:labelLoose++
    $script:looseList.Add(("{0} :{1}  ``{2}`` 在 {3} 行外（{4}）" -f $rel, $n, $r.name, $r.distance, $source))
    return
  }
  $script:labelMiss++
  $script:bad.Add("LABEL MISS -> $rel :$n 标签里的符号最近也在 $($r.distance) 行外（``$($r.name)``，上限 $LOOSE_LINES，来源 $source）；`n    该锚点很可能指到了无关代码 [$where]")
}

$labelRx = [regex]'`([A-Za-z0-9_./\\-]+\.(?:rs|js|mjs|md|json|yml|toml)):(\d+)(?:-\d+)?(?:\d+)?`\s*[（(]([^）)]{0,160})[）)]'
$bareLabelRx = [regex]'`:(\d+(?:/\d+)*)(?:-\d+)?`\s*[（(]([^）)]{0,160})[）)]'
$bareAnyRx = [regex]'`:(\d+(?:/\d+)*)(?:-\d+)?`'
$fileOnLineRx = [regex]'`([A-Za-z0-9_./\\-]+\.(?:rs|js|mjs|md|json|yml|toml)):'

foreach ($doc in $docs) {
  $docPath = Resolve-DocPath $doc
  foreach ($line in [System.IO.File]::ReadAllLines($docPath, [System.Text.Encoding]::UTF8)) {
    # 1) 完整锚点：`path:N`（label）
    foreach ($m in $labelRx.Matches($line)) {
      Test-AnchorLabel $m.Groups[1].Value.Trim() ([int]$m.Groups[2].Value) $m.Groups[3].Value $line $doc
    }
    # 2) 裸行号：`:N`（label）——文件取同一行内紧挨在它前面的完整锚点。
    #    这一类以前完全没做符号校验，F06 的 `:19273` 指到 `.pointer()` 就是这么漏的。
    if ($line -notmatch '`:\d') { continue }
    $labelled = @{}
    foreach ($m in $bareLabelRx.Matches($line)) { $labelled[$m.Index] = $m }
    # 逐个裸锚点：文件归属按「它前面最近的完整锚点」重算，不能整行共用一个。
    foreach ($m in $bareAnyRx.Matches($line)) {
      $before = $line.Substring(0, $m.Index)
      $lastFile = $null
      foreach ($fm in $fileOnLineRx.Matches($before)) { $lastFile = $fm.Groups[1].Value.Trim() }
      if (-not $lastFile) { continue }
      $ownLabel = ''
      if ($labelled.ContainsKey($m.Index)) { $ownLabel = $labelled[$m.Index].Groups[2].Value }
      foreach ($n in ($m.Groups[1].Value -split '/' | Where-Object { $_ -ne '' })) {
        Test-AnchorLabel $lastFile ([int]$n) $ownLabel $line $doc
      }
    }
  }
}
Write-Output "label-nearby: strict(<=$STRICT_LINES lines)=$labelStrict  loose(<=$LOOSE_LINES)=$labelLoose  miss=$labelMiss  skipped(no identifier)=$labelSkipped"
if ($VerbosePreference -ne 'SilentlyContinue' -or $env:MODE_PARITY_SHOW_LOOSE -eq '1') {
  foreach ($l in $looseList) { Write-Output "  [loose] $l" }
}
# skipped 的锚点等于「只过了弱校验（非空 + 非结构行）」。数量不为 0 时逐条列出来，
# 否则没人会去数，只会看到「skipped=54」这种没人负责的数字。
if ($skipList.Count -gt 0) {
  Write-Output "  [skipped] $($skipList.Count) 条锚点只有弱校验（标签与本行都没有可校验符号）："
  foreach ($s in $skipList) { Write-Output "    $s" }
}
# --- 锚点指纹锁：没有写符号名的锚点也要防漂移 -------------------------------
# 记录每个锚点当时指向那一行的内容摘要；下次跑若内容变了，说明代码移动过，
# 该锚点极可能已指错地方（前端重排 main.rs 行号时就是这样把一批锚点指到 `}` 上的）。
#
# 覆盖范围必须与「文档里实际存在的锚点集合」逐条对齐，否则 `drifted: 0` 是假的：
# 锁里只认自己记过的 key，文档新增的锚点不在锁里就**永不参与比对**。
# v0.5.1 就撞到过：锁停在 31f208e 的 296 条，文档 238 条里 80 条不在锁内，
# 前端新加的 :22258 / :19395 / :19678 / :24639 全都没被比对过，输出却是 drifted: 0。
# 现在两头都要校验：
#   - 文档里有、锁里没有 → UNLOCKED（锚点没纳入比对，等于没防漂移）
#   - 锁里有、文档里没有 → STALE（锁过期，说明没跟上文档，需要重跑 -UpdateAnchors）
$lockPath = Resolve-DocPath 'docs/parity-anchors.lock'
$lockLines = New-Object System.Collections.Generic.List[string]

# 与校验阶段完全相同的锚点扫描（含裸行号），保证「锁的键集合」== 「文档锚点集合」。
$lockSeen = @{}
function Add-LockEntry([string]$rel, [int]$start) {
  $full = Resolve-Path3 $rel
  if (-not $full) { return }
  $lines = [System.IO.File]::ReadAllLines($full)
  if ($start -lt 1 -or $start -gt $lines.Count) { return }
  $key = "$rel`:$start"
  if ($script:lockSeen.ContainsKey($key)) { return }
  $script:lockSeen[$key] = $true
  # 指纹：去空白 + 去掉行尾逗号，避免纯格式调整误报
  $sig = ($lines[$start - 1].Trim() -replace '\s+', ' ').TrimEnd(',')
  if ($sig.Length -gt 80) { $sig = $sig.Substring(0, 80) }
  $script:lockLines.Add("$key|$sig")
}

foreach ($doc in $docs) {
  $docPath = Resolve-DocPath $doc
  foreach ($line in [System.IO.File]::ReadAllLines($docPath, [System.Text.Encoding]::UTF8)) {
    # 1) 完整锚点：`path:N`（含 1-9 区间与 1/2/3 多位置写法）
    foreach ($m in $rx.Matches($line)) {
      $rel = $m.Groups[1].Value.Trim()
      Add-LockEntry $rel ([int]$m.Groups[2].Value)
      if ($m.Groups[3].Success) { Add-LockEntry $rel ([int]$m.Groups[3].Value) }
      foreach ($extra in ($m.Groups[4].Value -split '/' | Where-Object { $_ -ne '' })) {
        Add-LockEntry $rel ([int]$extra)
      }
    }
    # 2) 裸行号 `:N` / `:N/M`：文件取同一行内紧挨在它前面的完整锚点
    if ($line -notmatch '`:\d') { continue }
    foreach ($m in $bareAnyRx.Matches($line)) {
      $before = $line.Substring(0, $m.Index)
      $lastFile = $null
      foreach ($fm in $fileOnLineRx.Matches($before)) { $lastFile = $fm.Groups[1].Value.Trim() }
      if (-not $lastFile) { continue }
      foreach ($n in ($m.Groups[1].Value -split '/' | Where-Object { $_ -ne '' })) {
        Add-LockEntry $lastFile ([int]$n)
      }
    }
  }
}
$lockLines = @($lockLines | Sort-Object -Unique)
if ($UpdateAnchors) {
  # 合并写，不是覆盖写。锁里那些本次扫描够不到的条目（外部对照树没配
  # MODE_PARITY_DS_ROOT 时那 53 条 ds-bot/README 锚点）必须原样保留：
  # 覆盖写会在 CI 口径下把它们删掉，等下次带外部树跑就变成一片 unlocked 失败。
  # 保留的值仍是上次实时记录的指纹，够不到就没法验证它对不对——所以保留，
  # 由「stale-in-lock」计数如实报告，而不是假装它不存在。
  $kept = 0
  if (Test-Path $lockPath) {
    $merged = @{}
    foreach ($l in $lockLines) { $i = $l.IndexOf('|'); if ($i -gt 0) { $merged[$l.Substring(0, $i)] = $l.Substring($i + 1) } }
    foreach ($l in [System.IO.File]::ReadAllLines($lockPath, [System.Text.Encoding]::UTF8)) {
      $i = $l.IndexOf('|')
      if ($i -le 0) { continue }
      $k = $l.Substring(0, $i)
      if ($merged.ContainsKey($k)) { continue }
      if ($lockSeen.ContainsKey($k)) { continue }
      # 到这里有两种可能，必须分开处理，否则锁会越滚越大：
      #   (a) 文件在本机解析得到 -> 文档里这个锚点真的没了（被重排/删掉），作废丢弃；
      #   (b) 文件解析不到 -> 外部对照树没配，够不到 ≠ 不存在，原样保留。
      # 只按 "在不在 $lockSeen" 判断是不够的：$lockSeen 只记录**本次真扫到**的锚点，
      # 外部树够不到时也不会进去，会被误当成 (a) 删掉；(a)(b) 混在一起又会让
      # 重排后的旧行号永远赖在锁里（实测 451 条里有 39 条是这种死锚点）。
      $j = $k.LastIndexOf(':')
      $kf = if ($j -gt 0) { $k.Substring(0, $j) } else { $k }
      if (Resolve-Path3 $kf) { continue }
      $merged[$k] = $l.Substring($i + 1)
      $kept++
    }
    $lockLines = @($merged.Keys | Sort-Object | ForEach-Object { "$_|$($merged[$_])" })
  }
  # 写回**工作区**，不是基线目录。
  # $lockPath 现在走 Resolve-DocPath，在 -Baseline Head 下会指向临时目录，
  # 写到那里等于写完就被 Remove-BaselineDir 删掉，还报一句「已重建」——比不写更坏。
  # 锁是要提交的产物，只能落在仓库里。
  $lockOut = Join-Path $root 'docs/parity-anchors.lock'
  [System.IO.File]::WriteAllLines($lockOut, $lockLines, (New-Object System.Text.UTF8Encoding($false)))
  Write-Output "anchor lock rewritten: $($lockLines.Count) entries (本次可达 $($lockLines.Count - $kept)，保留 $kept 条够不到的) -> docs/parity-anchors.lock"
} elseif (Test-Path $lockPath) {
  $locked = @{}
  foreach ($l in [System.IO.File]::ReadAllLines($lockPath, [System.Text.Encoding]::UTF8)) {
    $i = $l.IndexOf('|')
    if ($i -gt 0) { $locked[$l.Substring(0, $i)] = $l.Substring($i + 1) }
  }
  $lockChecked = 0
  $lockDrift = 0
  $unlocked = New-Object System.Collections.Generic.List[string]
  foreach ($l in $lockLines) {
    $i = $l.IndexOf('|')
    $key = $l.Substring(0, $i)
    $sig = $l.Substring($i + 1)
    if (-not $locked.ContainsKey($key)) { $unlocked.Add($key); continue }
    $lockChecked++
    if ($locked[$key] -ne $sig) {
      $lockDrift++
      if ($lockDrift -le 5) {
        $bad.Add("ANCHOR MOVED -> $key 指向内容已变（原 [$($locked[$key])]，现 [$sig]）")
      }
    }
  }
  # 锁里多出来、文档里已不存在的条目：说明锁过期，不是错，但必须报告
  $stale = 0
  foreach ($k in $locked.Keys) { if (-not $lockSeen.ContainsKey($k)) { $stale++ } }
  Write-Output "anchor-fingerprint checked: $lockChecked  drifted: $lockDrift  unlocked: $($unlocked.Count)  stale-in-lock: $stale"
  # 未纳入比对 = 没有防漂移保护。只要有就判失败，并把前 10 条列出来，
  # 否则「drifted: 0」会一直被当成通过了。
  if ($unlocked.Count -gt 0) {
    $bad.Add("LOCK COVERAGE -> $($unlocked.Count) 个锚点不在 docs/parity-anchors.lock 里，没参与漂移比对；跑一次 -UpdateAnchors 并提交锁文件")
    foreach ($u in ($unlocked | Select-Object -First 10)) { Write-Output "    [unlocked] $u" }
  }
} else {
  Write-Output 'anchor-fingerprint: no lock file yet (run with -UpdateAnchors once)'
}# --- 状态计数一致性：汇总表数字必须等于明细行实际计数 -------------------------
# 明细行形如 `| A01 | ... | ✅ | ... |`；按行首 ID 归组，DOMAIN 表同样核对。
$matrix = [System.IO.File]::ReadAllLines((Resolve-DocPath 'docs/parity-matrix.md'), [System.Text.Encoding]::UTF8)
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
  Remove-BaselineDir
  Write-Output 'RESULT: all anchors resolve, summary counts consistent'
} else {
  Write-Output "RESULT: $($bad.Count) problem(s)"
  $bad | ForEach-Object { Write-Output "  $_" }
  Remove-BaselineDir
  exit 1
}
