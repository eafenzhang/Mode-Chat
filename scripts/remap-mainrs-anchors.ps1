$ErrorActionPreference = 'Stop'
$root = 'D:\ModeChat'
Set-Location $root

# 把文档里的 src/main.rs 锚点从 59c0c8e 平移到 ea955d5。
# 不能用「只认纯插入 hunk」的写法：这轮有一处 32 删 5 增的替换 hunk，
# 那种写法会把它当成 0 变化，导致后段所有锚点少移 27 行（实测 bot_details
# 文档 :20994、真值 :20975，正是差 19 = 27 - 8）。
$hunks = New-Object System.Collections.ArrayList
foreach ($line in (git diff -U0 59c0c8e 0dfbdca -- src/main.rs)) {
    if ($line -match '^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@') {
        $os = [int]$Matches[1]
        $oc = if ($Matches[2] -ne '') { [int]$Matches[2] } else { 1 }
        $ns = [int]$Matches[3]
        $nc = if ($Matches[4] -ne '') { [int]$Matches[4] } else { 1 }
        [void]$hunks.Add([pscustomobject]@{ OS = $os; OC = $oc; NS = $ns; NC = $nc })
    }
}
if ($hunks.Count -eq 0) { throw 'no hunks parsed' }
Write-Output "hunks: $($hunks.Count)"

$replacedNotes = New-Object System.Collections.Generic.List[string]

function Map-Line([int]$n) {
    $delta = 0
    foreach ($h in $hunks) {
        if ($h.OC -eq 0) {
            if ($n -gt $h.OS) { $delta += $h.NC }
            continue
        }
        if ($n -lt $h.OS) { break }
        if ($n -le ($h.OS + $h.OC - 1)) {
            $off = $n - $h.OS
            if ($off -ge $h.NC) { $off = [Math]::Max(0, $h.NC - 1) }
            return (@{ Line = $h.NS + $off + $delta; Replaced = $true })
        }
        $delta += ($h.NC - $h.OC)
    }
    return (@{ Line = $n + $delta; Replaced = $false })
}

$docs = @(
    'docs/parity-matrix.md',
    'docs/prd-0.5.md',
    'docs/parity-arch-notes.md',
    'docs/plans/2026-11-25-v0.5.0.md'
)

$fileRx = [regex]'`([A-Za-z0-9_./\\-]+\.(?:rs|js|mjs|md|json|yml|toml)):'
$numRx = [regex]':(\d+)(?:-(\d+))?((?:/\d+)*)'

$totalMapped = 0
foreach ($d in $docs) {
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
            # 归属：同一行内、紧挨在它前面最近的完整锚点文件名。
            # 注意要截到 $m.Index+1（含那个冒号）：$fileRx 的模式里冒号是结尾字符，
            # 截到 $m.Index 会把冒号切掉，导致本行第一个锚点永远认不到文件名
            # （实测就是这条 bug 让整批锚点「原地不动」）。
            $before = $line.Substring(0, [Math]::Min($line.Length, $m.Index + 1))
            $lastFile = $null
            foreach ($fm in $fileRx.Matches($before)) { $lastFile = $fm.Groups[1].Value.Trim() }
            if ($lastFile) { $curFile = $lastFile }
            $isMain = $false
            if ($m.Value.StartsWith(':')) {
                $isMain = ($curFile -eq 'src/main.rs' -or $curFile -eq 'main.rs')
            }
            if (-not $isMain) { continue }

            $n = [int]$m.Groups[1].Value
            $r = Map-Line $n
            if ($r.Replaced) {
                $replacedNotes.Add(("{0} :{1} -> :{2}  (落在替换区间，需人工确认)" -f $d, $n, $r.Line))
            }
            $newTok = ':' + $r.Line
            if ($m.Groups[2].Success) {
                $n2 = [int]$m.Groups[2].Value
                $r2 = Map-Line $n2
                $newTok += '-' + $r2.Line
            }
            if ($m.Groups[3].Success -and $m.Groups[3].Value.Length -gt 1) {
                foreach ($extra in ($m.Groups[3].Value -split '/' | Where-Object { $_ -ne '' })) {
                    $rx2 = Map-Line ([int]$extra)
                    $newTok += '/' + $rx2.Line
                }
            }
            [void]$sb.Append($line.Substring($pos, $m.Index - $pos))
            [void]$sb.Append($newTok)
            $pos = $m.Index + $m.Length
            $totalMapped++
        }
        if ($pos -gt 0) {
            [void]$sb.Append($line.Substring($pos))
            $newLine = $sb.ToString()
            if ($newLine -ne $line) { $lines[$i] = $newLine; $changed++ }
        }
    }
    if ($changed -gt 0) {
        [System.IO.File]::WriteAllLines($path, $lines, (New-Object System.Text.UTF8Encoding($false)))
    }
    Write-Output ("{0}: {1} lines changed" -f $d, $changed)
}
Write-Output "tokens remapped: $totalMapped"
Write-Output "--- replaced-region anchors (manual check) ---"
foreach ($n in $replacedNotes) { Write-Output "  $n" }
