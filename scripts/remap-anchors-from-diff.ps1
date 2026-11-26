# 通用版：把文档里的 src/main.rs 锚点按 git diff 的 hunk 表平移。
# 与 remap-mainrs-anchors.ps1 的区别：基线/目标是参数，不写死 commit。
#
#   用法： powershell -File scripts\remap-anchors-from-diff.ps1 -From HEAD -To WORKTREE [-Write]
#
# 为什么必须按 hunk 表算、不能只做「净减 N 行」：
#   D-CUT 是 84 个 hunk 混着删除与插入，每段之后的行号偏移都不同。
#   只做总量相减会让删除区之后的所有锚点整体错位（v0.5.1 那次少移 27 行就是这么来的）。
#
# 落在「纯删除区间」里的锚点无行可指 —— 脚本不猜，打印 DELETED 交人工重指，
# 这类必须由人按语义挑一行新代码，机器平移只会把锚点挪到无关行上。
param(
    [string]$From = 'HEAD',
    [string]$To = 'WORKTREE',
    [string[]]$Docs = @(
        'docs/parity-matrix.md',
        'docs/prd-0.5.md',
        'docs/prd-0.6.md',
        'docs/parity-arch-notes.md',
        'docs/plans/2026-11-25-v0.5.0.md'
    ),
    [switch]$Write
)
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# ---- 1. 解析 hunk ---------------------------------------------------------
$hunks = New-Object System.Collections.ArrayList
$diffArgs = if ($To -eq 'WORKTREE') { @('diff', '-U0', $From, '--', 'src/main.rs') }
            else { @('diff', '-U0', $From, $To, '--', 'src/main.rs') }
foreach ($line in (& git @diffArgs)) {
    if ($line -match '^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@') {
        $os = [int]$Matches[1]
        # 判断「会不会存在第 N 组」必须用 ContainsKey，不能用 -ne ''，也不能用 .Success：
        #   - hunk 头写 `@@ -8842 +8845 @@`（省略 ,N）时，$Matches 里根本没有第 2 组，
        #     $Matches[2] 取出来是 $null，而 PowerShell 里 `$null -ne ''` 为 **True**，
        #     于是走了「有值」分支，[int]$null = 0 —— 替换型 hunk 被当成「删 0 增 0」整个跳过。
        #   - $Matches 是 hashtable，取出的元素是 string，取 `.Success` 恒为 $null（同样恒假）。
        # 实测：本仓 +12/-4 的 diff 曾被解析成「删 0 增 8」，22 个受影响的锚点原地不动，
        # 表现为 drifted=115，看起来像「代码动了锁没跟上」，实际是这里丢了行数。
        # 省略 ,N 按 git 语义就是 1 行。
        $oc = if ($Matches.ContainsKey(2) -and $Matches[2] -ne '') { [int]$Matches[2] } else { 1 }
        $ns = [int]$Matches[3]
        $nc = if ($Matches.ContainsKey(4) -and $Matches[4] -ne '') { [int]$Matches[4] } else { 1 }
        [void]$hunks.Add([pscustomobject]@{ OS = $os; OC = $oc; NS = $ns; NC = $nc })
    }
}
if ($hunks.Count -eq 0) { throw "no hunks parsed ($From -> $To)" }
Write-Output "hunks: $($hunks.Count)  删 $(($hunks | Measure-Object -Property OC -Sum).Sum)  增 $(($hunks | Measure-Object -Property NC -Sum).Sum)"

# ---- 2. 行号映射 ----------------------------------------------------------
# 返回 @{ Line; State }  State: kept / shifted / deleted
function Map-Line([int]$n) {
    $delta = 0
    foreach ($h in $hunks) {
        if ($h.OC -eq 0) {                       # 纯插入：插在第 OS 行之后
            if ($n -gt $h.OS) { $delta += $h.NC }
            continue
        }
        if ($n -lt $h.OS) { break }              # 落在本 hunk 之前，后面都不用看
        if ($n -le ($h.OS + $h.OC - 1)) {         # 落在被删掉的区间里
            return @{ Line = $null; State = 'deleted' }
        }
        $delta += ($h.NC - $h.OC)
    }
    return @{ Line = ($n + $delta); State = $(if ($delta -eq 0) { 'kept' } else { 'shifted' }) }
}

# ---- 3. 扫文档 ------------------------------------------------------------
$fileRx = [regex]'`([A-Za-z0-9_./\\-]+\.(?:rs|js|mjs|md|json|yml|toml)):'
$numRx = [regex]':(\d+)(?:-(\d+))?((?:/\d+)*)'
$deletedNotes = New-Object System.Collections.Generic.List[string]
$totalMapped = 0
$totalDeleted = 0
# 改写前先量一次，末尾自检用同一个函数再量一次（口径必须一致，见第 4 节）。
function Measure-MainTokens {
    $n = 0
    foreach ($d in $Docs) {
        $path = Join-Path $root $d
        if (-not (Test-Path $path)) { continue }
        foreach ($line in [System.IO.File]::ReadAllLines($path, [System.Text.Encoding]::UTF8)) {
            if ($line -notmatch 'main\.rs:') { continue }
            $n += $numRx.Matches($line).Count
        }
    }
    return $n
}


$beforeTotal = Measure-MainTokens


foreach ($d in $Docs) {
    $path = Join-Path $root $d
    if (-not (Test-Path $path)) { continue }
    $lines = [System.IO.File]::ReadAllLines($path, [System.Text.Encoding]::UTF8)
    $changed = 0
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        if ($line -notmatch 'main\.rs:') { continue }

        $sb = New-Object System.Text.StringBuilder
        $pos = 0
        $curFile = $null
        foreach ($m in $numRx.Matches($line)) {
            # 归属：同一行内紧挨在它前面最近的完整锚点文件名（截到冒号，否则首个锚点认不到文件）
            $before = $line.Substring(0, [Math]::Min($line.Length, $m.Index + 1))
            $lastFile = $null
            foreach ($fm in $fileRx.Matches($before)) { $lastFile = $fm.Groups[1].Value.Trim() }
            if ($lastFile) { $curFile = $lastFile }
            if ($curFile -ne 'src/main.rs' -and $curFile -ne 'main.rs') { continue }

            $n = [int]$m.Groups[1].Value
            $r = Map-Line $n
            if ($r.State -eq 'deleted') {
                $deletedNotes.Add(("{0}:{1}  :{2}  <- 该行已被删除，需人工重指" -f $d, ($i + 1), $n))
                $totalDeleted++
                continue
            }
            $newTok = ':' + $r.Line
            if ($m.Groups[2].Success) {
                $r2 = Map-Line ([int]$m.Groups[2].Value)
                if ($r2.State -ne 'deleted') { $newTok += '-' + $r2.Line }
            }
            if ($m.Groups[3].Success -and $m.Groups[3].Value.Length -gt 1) {
                foreach ($extra in ($m.Groups[3].Value -split '/' | Where-Object { $_ -ne '' })) {
                    $rx2 = Map-Line ([int]$extra)
                    if ($rx2.State -ne 'deleted') { $newTok += '/' + $rx2.Line }
                }
            }
            [void]$sb.Append($line.Substring($pos, $m.Index - $pos))
            [void]$sb.Append($newTok)
            $pos = $m.Index + $m.Length
            $totalMapped++
        }
        if ($pos -gt 0) {
            [void]$sb.Append($line.Substring($pos))
            if ($sb.ToString() -ne $line) { $lines[$i] = $sb.ToString(); $changed++ }
        }
    }
    if ($changed -gt 0) {
        if ($Write) {
            [System.IO.File]::WriteAllLines($path, $lines, (New-Object System.Text.UTF8Encoding($false)))
            Write-Output ("{0}: {1} lines rewritten" -f $d, $changed)
        } else {
            Write-Output ("{0}: {1} lines WOULD change (dry-run，加 -Write 才落盘)" -f $d, $changed)
        }
    }
}

Write-Output "tokens remapped: $totalMapped"
Write-Output "anchors pointing at deleted lines: $totalDeleted"

# ---- 4. 自检：token 数必须守恒 ------------------------------------------
# 这一轮已经踩过三次「看起来绿了、实际少扫」，所以脚本自己也要守这条：
#   重指只允许改行号，**不允许改变锚点 token 的总数**。
# 少掉或多出都说明有 token 被吃掉/拼坏（正则漏了某种写法、区间被压平、多位置写法丢了），
# 而这类问题在输出里完全看不出来，所以必须 EXIT=1。
#
# 统计必须用**同一个函数**在改写前后各算一次。最初版本用扫描阶段的 $tokensSeen 当 before，
# 它按「当前归属文件」过滤（一行里第一个锚点若是别的文件，后面的裸行号就归那边），
# 而 after 用「行里有没有 main.rs:」过滤 —— 两个口径不同，实测差 123 个，天天误报。
# 判据只能比「同一个量」的前后值，不能比两个定义不同的量。
# 被删掉的锚点本来就该少一个，所以从 before 里扣掉。
# 被删掉的锚点本来就该少一个，所以从 before 里扣掉。
$afterTotal = Measure-MainTokens
$expected = $beforeTotal - $totalDeleted
# 注意：这两个量都是「本次运行前后」的，所以它只能证明**本脚本没有吃掉 token**，
# 不能证明「别人手工删掉一个锚点」——那种删除让 before 和 after 一起变小，仍然守恒。
# 我用「先删一个 token 再跑」做负向用例，实测这样是拦不住的（SELF-CHECK OK）。
# 真正能拦的是指纹锁那一侧的 LOCK COVERAGE：锚点不再被扫描时它报 unlocked/stale，
# 所以这里只负责守住「重指动作本身不丢 token」这一条，别把它当通用防线。
if ($afterTotal -ne $expected) {
    Write-Output "SELF-CHECK FAILED: 锚点 token 数不守恒（差 $($afterTotal - $expected) 个）。"
    Write-Output "  可能原因：正则漏掉某种写法、或重指时把区间/多位置写法拼坏了。"
    Write-Output "  不要提交这次改写；先逐条比对文档，确认少掉/多出的 token 是哪一个。"
    foreach ($n in $deletedNotes) { Write-Output "  [DELETED] $n" }
    exit 1
}
Write-Output "SELF-CHECK OK: 锚点 token 数守恒"
foreach ($n in $deletedNotes) { Write-Output "  [DELETED] $n" }
