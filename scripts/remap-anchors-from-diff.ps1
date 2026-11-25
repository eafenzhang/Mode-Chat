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
        $oc = if ($Matches[2] -ne '') { [int]$Matches[2] } else { 1 }
        $ns = [int]$Matches[3]
        $nc = if ($Matches[4] -ne '') { [int]$Matches[4] } else { 1 }
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
foreach ($n in $deletedNotes) { Write-Output "  [DELETED] $n" }
