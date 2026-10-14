param(
    [Parameter(Mandatory=$true)][string]$Executable,
    [Parameter(Mandatory=$true)][string]$Workspace,
    [Parameter(Mandatory=$true)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
$binaryPath = (Resolve-Path -LiteralPath $Executable).Path
$workspacePath = (Resolve-Path -LiteralPath $Workspace).Path
$outputPath = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Path $outputPath -Force | Out-Null
$binaryHash = (Get-FileHash -LiteralPath $binaryPath -Algorithm SHA256).Hash.ToLowerInvariant()
$captures = [System.Collections.Generic.List[object]]::new()
function Quote-NativeArgument([string]$value) {
    $escaped = [regex]::Replace($value, '(\\*)"', {
        param($match)
        $match.Groups[1].Value + $match.Groups[1].Value + '\"'
    })
    $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
    return '"' + $escaped + '"'
}
foreach ($captureName in @('models','settings','approvals','chat','files','tasks','sessions','sessions-narrow','chat-narrow','models-narrow','settings-narrow','tasks-narrow','chat-activity','chat-management','chat-activity-narrow','chat-management-narrow','archives','archives-narrow','chat-delete-preview','chat-delete-preview-narrow','tasks-delete-preview','tasks-delete-preview-narrow','archives-trash','archives-trash-narrow')) {
    $screen = $captureName -replace '-narrow$', '' -replace '-(activity|management|delete-preview|trash)$', ''
    $profile = Join-Path $outputPath "$captureName-profile"
    $capture = Join-Path $outputPath "$captureName.png"
    $nativeArguments = @('--workspace',$workspacePath,'--read-only','--profile',$profile,'--smoke','--smoke-screen',$screen,'--screenshot',$capture)
    if ($captureName.EndsWith('-narrow')) { $nativeArguments += '--smoke-narrow' }
    if ($captureName -match '-(activity|management)(-narrow)?$') { $nativeArguments += @('--smoke-dock', $Matches[1]) }
    if ($captureName -match '-delete-preview(-narrow)?$') { $nativeArguments += @('--smoke-dialog', 'delete-preview') }
    if ($captureName -match '-trash(-narrow)?$') { $nativeArguments += @('--smoke-dialog', 'trash') }
    $argumentText = ($nativeArguments | ForEach-Object { Quote-NativeArgument $_ }) -join ' '
    $process = Start-Process -FilePath $binaryPath -ArgumentList $argumentText -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $outputPath "$captureName.stdout.log") -RedirectStandardError (Join-Path $outputPath "$captureName.stderr.log")
    $null = $process.Handle
    if (-not $process.WaitForExit(60000)) { throw "Capture $screen exceeded deadline; pid $($process.Id) retained for inspection" }
    if ($null -eq $process.ExitCode -or $process.ExitCode -ne 0) { throw "Capture $screen failed: $($process.ExitCode)" }
    $reportPath = Join-Path $profile 'native-window-smoke.json'
    $report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
    if ($report.error -or -not $report.orderly_shutdown -or -not $report.screenshot_received -or $report.rendered_sections.Count -ne 23 -or -not (Test-Path -LiteralPath $capture)) { throw "Capture $screen has incomplete window evidence" }
    if ($report.window_chrome.integrated_titlebar -ne $true -or $report.window_chrome.platform -ne 'Windows' -or $report.window_chrome.requested_native_decorations -ne $false -or $report.window_chrome.close_uses_application_lifecycle -ne $true) { throw "Capture $screen has incomplete custom frame configuration" }
    if ($captureName -match '-delete-preview(-narrow)?$') {
        if ($report.native_management.dialog_open -ne $true -or $report.native_management.preview_received -ne $true -or $report.native_management.preview_pending -or $report.native_management.action_pending -or $report.native_management.error) { throw "Capture $captureName has incomplete read-only deletion preview evidence" }
    }
    if ($screen -eq 'sessions' -and $report.native_management.monitor_received -ne $true) { throw 'Sessions monitor did not receive a native result' }
    if ($captureName -match '-trash(-narrow)?$' -and $report.native_management.trash_received -ne $true) { throw 'Conversation trash did not receive a native result' }
    if ($captureName -match '-(activity|management)(-narrow)?$') {
        $expectedTab = $Matches[1]
        $expectedWidth = if ($captureName.EndsWith('-narrow')) { 900.0 } else { 1320.0 }
        if (-not $report.drawer.open -or $report.drawer.tab -ne $expectedTab -or
            $null -eq $report.drawer.visible_body_height -or [double]$report.drawer.visible_body_height -lt 59.0 -or
            [Math]::Abs([double]$report.drawer.logical_rect.width - $expectedWidth) -gt 1.0) {
            throw "Capture $captureName has insufficient full-width drawer content"
        }
    }
    [void]$captures.Add(@{screen=$screen;variant=$captureName;path=$capture;sha256=(Get-FileHash -LiteralPath $capture -Algorithm SHA256).Hash.ToLowerInvariant();window_report=$reportPath;exit_code=$process.ExitCode;orderly_shutdown=$report.orderly_shutdown})
}
if ((Get-FileHash -LiteralPath $binaryPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $binaryHash) { throw 'Executable changed during captures' }
$report = @{passed=$true;executable=$binaryPath;executable_sha256=$binaryHash;workspace=$workspacePath;read_only=$true;captures=$captures.ToArray()}
$report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $outputPath 'capture-review-result.json') -Encoding UTF8
Write-Output "Native capture review PASS: $outputPath"
