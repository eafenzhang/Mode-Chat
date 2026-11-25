# Step 5: remove genuine duplicate (settings_nav) and annotate feature-bearing orphans.
# ASCII-only on purpose: Windows PowerShell 5.1 reads BOM-less .ps1 as ANSI.
[CmdletBinding()]
param([switch]$Apply, [string]$Path = 'D:\ModeChat\src\main.rs')
$ErrorActionPreference = 'Stop'
$L = [System.IO.File]::ReadAllLines($Path, [System.Text.Encoding]::UTF8)
Write-Output ("before: {0} lines" -f $L.Count)

# one deletion: classic settings sidebar (wechat shell draws its own nav)
$delStart = 8827
$delEnd = 8929
if (-not $L[$delStart-1].Contains('fn settings_nav(&self, cx: &mut Context<Self>) -> gpui::Div {')) {
  Write-Output 'ANCHOR FAIL settings_nav'; exit 1
}

# eight annotations, applied while walking the file top-down
$adds = @(
  @(599,  'fn save_browser_recents(bot_id: &str, recents: &[String]) {', '// v0.7 decision: browser recents write-back, no wechat-shell entry point yet.'),
  @(1001, 'struct PluginSwitchConfirm {', '// v0.7 decision: workspace plugin switch confirm, no wechat-shell write entry yet.'),
  @(3190, 'browser_url_input: Entity<SimpleInput>,', '    // v0.7 decision: browser URL input, no wechat-shell entry point yet.'),
  @(8667, 'fn browser_panel_section(&self, cx: &mut Context<Self>)', '    // v0.7 decision: browser URL / recents card, no wechat-shell entry point yet.'),
  @(8765, 'fn record_browser_open(&mut self, bot_id: &str, url: &str)', '    // v0.7 decision: only called by browser_panel_section.'),
  @(16395, 'fn apply_plugin_switch(&mut self, confirm: &PluginSwitchConfirm', '    // v0.7 decision: workspace .mode/config.json switch writer.'),
  @(16423, 'fn open_plugin_confirm_dialog(', '    // v0.7 decision: switch confirm dialog.'),
  @(20364, 'RenameSession {', '    // v0.7 decision: session rename, no wechat-shell entry point yet.')
)

$bad = 0
foreach ($a in $adds) {
  if (-not $L[$a[0]-1].Contains($a[1])) { Write-Output ("ANCHOR FAIL {0} '{1}'" -f $a[0], $a[1]); $bad++ }
}
if ($bad -gt 0) { Write-Output ("$bad anchor(s) failed -- nothing written"); exit 1 }
if (-not $Apply) { Write-Output ("1 deletion ({0}-{1}) + {2} annotations verified; DRY RUN" -f $delStart, $delEnd, $adds.Count); exit 0 }

$out = New-Object System.Collections.Generic.List[string]
for ($i = 0; $i -lt $L.Count; $i++) {
  $ln = $i + 1
  if ($ln -ge $delStart -and $ln -le $delEnd) { continue }
  foreach ($a in $adds) {
    if ($a[0] -eq $ln) {
      $indent = [regex]::Match($L[$i], '^\s*').Value
      $out.Add($indent + '#[allow(dead_code)] ' + $a[2])
    }
  }
  $out.Add($L[$i])
}
$fin = New-Object System.Collections.Generic.List[string]
$blank = 0
foreach ($l in $out) {
  if ($l.Trim().Length -eq 0) { $blank++; if ($blank -gt 1) { continue } } else { $blank = 0 }
  $fin.Add($l)
}
[System.IO.File]::WriteAllText($Path, (($fin -join "`r`n") + "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
Write-Output ("WROTE {0}  {1} -> {2}" -f $Path, $L.Count, $fin.Count)
