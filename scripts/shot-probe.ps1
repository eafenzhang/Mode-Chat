# 截图自查探针：给「4b / 4c 三连拍」这类 UI 状态帧用的客观量测。
#
# 为什么需要它：这一轮反复出错的根源不是人看错了，是自查脚本把坐标写死了。
# 同一个 x 在 1294 宽的窗口里属于右栏，在 1614 宽的窗口里属于中栏——判据里
# 写死一个绝对值，换尺寸就换含义。本脚本不假设任何固定坐标，全部按图宽推导。
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\shot-probe.ps1 docs\testing\shots\266-4b-3-reopened.png
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\shot-probe.ps1 <目录通配符>
#
# 输出四组量：
#   size          图宽高
#   panel         右栏是否存在、左边界（按「最右一列仍密集出现 250 灰」定位）
#   exactGreen    右栏区域内的精确 0x07C160 像素数（卡片描边色；就绪 badge 的
#                 浅绿 tint 217,255,217 不算，所以这个数不随阈值漂移）
#   leftBand      左栏选中绿带的 y 区间（用来判「当前选中哪个 Bot」，因为
#                 「12」和「13」字符宽度相同，量标题宽度区分不出来）
# ValueFromRemainingArguments：这样一次可以传多个路径，
# 也可以传逗号分隔的一串（从 cmd 调用时常见）。
param(
  [Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)]
  [string[]]$Path
)

# 兼容「一次给一个逗号串」：拆开再逐个解析。
$Path = @($Path | ForEach-Object { $_ -split ',' } | Where-Object { $_.Trim() -ne '' })

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

# 右栏基准宽度（ui::PANEL_WIDTH），用于限定搜索范围，不用于定位。
$PANEL_WIDTH = 320
# 250 灰是这个主题下右栏卡片的底色；一列里出现这么多次才算「这一列在右栏里」。
$COLUMN_HIT_MIN = 60

function Probe-One([string]$file) {
  $bmp = [System.Drawing.Bitmap]::new((Resolve-Path $file).Path)
  try {
    $w = $bmp.Width
    $h = $bmp.Height
    $y0 = 100
    $y1 = [Math]::Max($y0 + 1, $h - 147)   # 避开标题栏与底部输入框

    # --- 1) 定位右栏：从右往左，找第一列「250 灰」密集的竖线 ---------------
    # 只扫右侧 45% 宽度，中栏不可能那么靠右，省时间也不会误判。
    $panelLeft = -1
    $panelCol = -1
    for ($x = $w - 1; $x -ge [int]($w * 0.55); $x -= 1) {
      $hits = 0
      for ($y = $y0; $y -le $y1; $y += 4) {
        $c = $bmp.GetPixel($x, $y)
        if ($c.R -eq 250 -and $c.G -eq 250 -and $c.B -eq 250) { $hits++ }
      }
      if ($hits -ge ($COLUMN_HIT_MIN / 4)) { $panelCol = $x; break }
    }
    $panelPresent = $panelCol -ge 0
    if ($panelPresent) {
      # 右栏左边界：从找到的那一列往左，右栏实际占 PANEL_WIDTH 宽（含 12px 边距）。
      $panelLeft = [Math]::Max(0, $panelCol - $PANEL_WIDTH - 12)
    }

    # --- 2) 右栏内的精确描边色计数 ----------------------------------------
    # 只数 0x07C160 精确值：就绪 badge 用 rgb(217,255,217) 浅 tint，
    # 用「绿像素 < N」这种阈值写法会把它算进去，换个阈值就换个结论。
    $exactGreen = 0
    if ($panelPresent) {
      for ($y = 0; $y -lt $h; $y++) {
        for ($x = $panelLeft; $x -lt $w; $x++) {
          $c = $bmp.GetPixel($x, $y)
          if ($c.R -eq 7 -and $c.G -eq 193 -and $c.B -eq 96) { $exactGreen++ }
        }
      }
    }

    # --- 3) 左栏选中绿带的 y 区间 ----------------------------------------
    # 「选中 12」和「选中 13」在标题文字上量不出差别（都是两个字符），
    # 但选中项有整行绿底，行位置不同 —— 用它判当前在哪个会话。
    $bandTop = -1
    $bandBottom = -1
    for ($y = 0; $y -lt $h; $y++) {
      $rowHits = 0
      for ($x = 0; $x -lt [Math]::Min(300, $w); $x += 2) {
        $c = $bmp.GetPixel($x, $y)
        if ($c.R -eq 7 -and $c.G -eq 193 -and $c.B -eq 96) { $rowHits++ }
      }
      if ($rowHits -ge 20) {
        if ($bandTop -lt 0) { $bandTop = $y }
        $bandBottom = $y
      }
    }

    [pscustomobject]@{
      File        = Split-Path -Leaf $file
      Size        = "${w}x${h}"
      Panel       = if ($panelPresent) { "present (left=$panelLeft)" } else { 'ABSENT' }
      ExactGreen  = $exactGreen
      LeftBand    = if ($bandTop -ge 0) { "y=$bandTop..$bandBottom" } else { 'none' }
    }
  } finally {
    $bmp.Dispose()
  }
}

$files = @()
foreach ($p in $Path) {
  if (Test-Path $p -PathType Container) {
    $files += Get-ChildItem -LiteralPath $p -Filter *.png -File | ForEach-Object { $_.FullName }
  } else {
    $resolved = Get-ChildItem -Path $p -File -ErrorAction SilentlyContinue
    if ($resolved) { $files += $resolved | ForEach-Object { $_.FullName } }
  }
}
if ($files.Count -eq 0) {
  # Write-Output 是 cmdlet，'a' + $b 会被当成两个独立参数逐字打印（还会带个 '+'），
  # 必须用括号把表达式收成一个字符串。
  Write-Output ('not found: ' + ($Path -join ', '))
  exit 2
}

$results = @()
foreach ($f in $files) {
  $r = Probe-One $f
  $results += $r
  Write-Output ('{0,-34} {1,-10} panel={2,-22} exactGreen={3,-6} leftBand={4}' -f `
      $r.File, $r.Size, $r.Panel, $r.ExactGreen, $r.LeftBand)
}

Write-Output ''
Write-Output '判读（4b / 4c 三连拍）：'
Write-Output '  4b-1 亮       panel=present  exactGreen > 2000'
Write-Output '  4b-2 折叠     panel=ABSENT   exactGreen = 0'
Write-Output '  4b-3 展开     panel=present  exactGreen = 0     ← 与 4b-2 的区分点是 panel'
Write-Output '  4c-1 亮       panel=present  exactGreen > 2000  leftBand 在 12 那一行'
Write-Output '  4c-2/3 无高亮  panel=present  exactGreen = 0     leftBand 分别在 13 / 12'
Write-Output ''
Write-Output '注意：panel 与 exactGreen 必须一起看。只看 exactGreen = 0，'
Write-Output '      分不清「折叠」和「展开后已清」——这一轮 264-4b-3 就是这样误判的。'
