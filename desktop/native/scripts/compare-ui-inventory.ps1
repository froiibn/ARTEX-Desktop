param(
    [Parameter(Mandatory=$true)][string]$Baseline,
    [Parameter(Mandatory=$true)][string]$VerificationResult
)
$ErrorActionPreference = 'Stop'
$baselinePath = (Resolve-Path -LiteralPath $Baseline).Path
$verificationPath = (Resolve-Path -LiteralPath $VerificationResult).Path
$before = Get-Content -LiteralPath $baselinePath -Raw | ConvertFrom-Json
$after = Get-Content -LiteralPath $verificationPath -Raw | ConvertFrom-Json
if (-not $before.passed -or -not $after.passed) { throw 'Only passing verification reports can be compared' }
function ActionSignatures($report) {
    @($report.source_management_actions | ForEach-Object {
        @($_.key, $_.method, $_.section, [bool]$_.mutation) | ConvertTo-Json -Compress
    } | Sort-Object)
}
$beforeActions = ActionSignatures $before
$afterActions = ActionSignatures $after
if ($beforeActions.Count -ne 168 -or $afterActions.Count -ne 168 -or
    @($beforeActions | Sort-Object -Unique).Count -ne 168 -or
    @($afterActions | Sort-Object -Unique).Count -ne 168 -or
    @(Compare-Object $beforeActions $afterActions -CaseSensitive).Count -ne 0) {
    throw 'Existing management action identity, method, section or mutation flag changed'
}
$beforeSections = @($before.rendered_sections | Sort-Object)
$afterSections = @($after.rendered_sections | Sort-Object)
if ($beforeSections.Count -ne 23 -or $afterSections.Count -ne 23 -or
    @($beforeSections | Sort-Object -Unique).Count -ne 23 -or
    @($afterSections | Sort-Object -Unique).Count -ne 23 -or
    @(Compare-Object $beforeSections $afterSections -CaseSensitive).Count -ne 0) {
    throw 'Existing section identity changed'
}
$report = [ordered]@{
    passed = $true
    completed_utc = [DateTime]::UtcNow.ToString('o')
    baseline = $baselinePath
    baseline_sha256 = (Get-FileHash -LiteralPath $baselinePath -Algorithm SHA256).Hash.ToLowerInvariant()
    verification_result = $verificationPath
    verification_sha256 = (Get-FileHash -LiteralPath $verificationPath -Algorithm SHA256).Hash.ToLowerInvariant()
    unchanged_action_count = $afterActions.Count
    unchanged_section_count = $afterSections.Count
    comparison_fields = @('key','method','section','mutation')
    sections = $afterSections
    action_signatures = $afterActions
    qualification = 'Exact inventory preservation; not proof of every original interaction or cross-platform deployment'
}
$reportPath = Join-Path (Split-Path -Parent $verificationPath) 'ui-parity.json'
$report | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $reportPath -Encoding UTF8
Write-Output "Native UI inventory PASS: $reportPath"
