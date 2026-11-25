# Mark the three feature-bearing orphan groups as intentionally kept.
# Each anchor is verified by content before writing; inserts are applied bottom-up.
[CmdletBinding()]
param([switch]$Apply, [string]$Path = 'D:\ModeChat\src\main.rs')
$ErrorActionPreference = 'Stop'
$L = [System.IO.File]::ReadAllLines($Path, [System.Text.Encoding]::UTF8)
Write-Output ("before: {0} lines" -f $L.Count)

$NOTE = '    // D-CUT 遗留：入口原来只在 classic 外壳里，微信壳没有等价物。'
$adds = @(
  @(599, 'fn save_browser_recents(bot_id: &str, recents: &[String]) {', '#[allow(dead_code)] // 浏览分页「最近打开」的写回；微信壳暂无该入口（v0.7 接回或删）。'),
  @(8667, 'fn browser_panel_section(&self, cx: &mut Context<Self>)', '    #[allow(dead_code)] // 浏览分页的 URL/最近记录卡；微信壳暂无该入口（v0.7 接回或删）。'),
  @(8765, 'fn record_browser_open(&mut self, bot_id: &str, url: &str)', '    #[allow(dead_code)] // 同上：只被 browser_panel_section 调用。'),
  @(3190, 'browser_url_input: Entity<SimpleInput>,', '    #[allow(dead_code)] // 同上：浏览分页 URL 输入框。'),
  @(1001, 'struct PluginSwitchConfirm {', '#[allow(dead_code)] // 插件开关二次确认；微信壳设置页暂无写入入口（v0.7 接回或删）。'),
  @(16395, 'fn apply_plugin_switch(&mut self, confirm: &PluginSwitchConfirm', '    #[allow(dead_code)] // 同上：workspace .mode/config.json 的开关写入。'),
  @(16423, 'fn open_plugin_confirm_dialog(', '    #[allow(dead_code)] // 同上：二次确认弹层。'),
  @(20364, 'RenameSession {', '    #[allow(dead_code)] // 会话改名；微信壳会话行暂无「改名」入口（v0.7 接回或删）。')
)

$bad = 0
foreach ($a in $adds) {
  if (-not $L[$a[0]-1].Contains($a[1])) { Write-Output ("ANCHOR FAIL {0} '{1}'" -f $a[0], $a[1]); $bad++ }
}
if ($bad -gt 0) { Write-Output ("$bad failed"); exit 1 }
Write-Output ("all {0} anchors ok" -f $adds.Count)
if (-not $Apply) { Write-Output 'DRY RUN'; exit 0 }

$out = New-Object System.Collections.Generic.List[string]
for ($i = 0; $i -lt $L.Count; $i++) {
  foreach ($a in $adds) { if ($a[0] - 1 -eq $i) { $out.Add($a[2]) } }
  $out.Add($L[$i])
}
[System.IO.File]::WriteAllText($Path, (($out -join "`r`n") + "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
Write-Output ("WROTE {0}  {1} -> {2}" -f $Path, $L.Count, $out.Count)
