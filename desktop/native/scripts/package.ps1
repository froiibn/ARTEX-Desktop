param(
    [Parameter(Mandatory=$true)][string]$VerificationResult,
    [ValidatePattern('^[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?$')][string]$Version = '0.2.0-preview.3',
    [string]$LicenseCache
)
$ErrorActionPreference = 'Stop'
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..'))
Set-Location -LiteralPath $repoRoot
$verificationPath = (Resolve-Path -LiteralPath $VerificationResult).Path
$verification = Get-Content -LiteralPath $verificationPath -Raw -Encoding UTF8 | ConvertFrom-Json
if (-not $verification.passed -or -not $verification.full_regression) { throw 'A passing native full-regression report is required' }
$visualReviewPath = Join-Path (Split-Path -Parent $verificationPath) 'visual-review.json'
if (Test-Path -LiteralPath $visualReviewPath) {
    $visualReview = Get-Content -LiteralPath $visualReviewPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($visualReview.accepted -ne $true) { throw 'Native visual review rejected this build; repair and recapture before packaging' }
}
foreach ($source in $verification.source_files) {
    if ((Get-FileHash -LiteralPath $source.path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $source.sha256) { throw "Source changed since verification: $($source.path)" }
}
if (& git status --porcelain) { throw 'Commit the verified source before creating a release package' }
$sourceCommit = (& git rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Source commit unavailable' }
$rustVersion = (& rustc -vV) -join "`n"
if ($rustVersion -notmatch '(?m)^host: x86_64-pc-windows-gnu$') { throw 'This package recipe requires the Windows GNU Rust toolchain; other targets need their own runtime provenance' }
$target = 'x86_64-pc-windows-gnu'
$runId = [guid]::NewGuid().ToString('N')
$artifactRoot = Join-Path $repoRoot "desktop/dist/native-windows-$Version-$runId"
$package = Join-Path $artifactRoot 'ARTEX-Windows-x64'
$audit = Join-Path $artifactRoot 'audit'
if (-not $LicenseCache) { $LicenseCache = Join-Path $repoRoot 'desktop/dist/license-cache' }
New-Item -ItemType Directory -Path $package,$audit | Out-Null
function ConvertTo-NativeArgument([string]$value) {
    $escaped = [regex]::Replace($value, '(\\*)"', { param($match) $match.Groups[1].Value + $match.Groups[1].Value + '\"' })
    $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
    return '"' + $escaped + '"'
}
function Start-PackageProcess([string]$file, [string[]]$nativeArguments, [string]$name) {
    $argumentText = ($nativeArguments | ForEach-Object { ConvertTo-NativeArgument $_ }) -join ' '
    $child = Start-Process -FilePath $file -ArgumentList $argumentText -WorkingDirectory $repoRoot -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $audit "$name.stdout.log") -RedirectStandardError (Join-Path $audit "$name.stderr.log")
    $null = $child.Handle
    return $child
}
function Confirm-PackageProcess($child, [string]$name) {
    $child.WaitForExit()
    if ($null -eq $child.ExitCode -or $child.ExitCode -ne 0) { throw "$name failed or has no confirmed exit code; see local audit logs" }
}
$linkMap = Join-Path $audit 'native-link.map'
# A stable Cargo flag permits dependency reuse while the final native link regenerates the map.
$buildLinkMap = Join-Path $repoRoot 'desktop/target/native-release-link.map'
$oldTarget = $env:CARGO_TARGET_DIR
$oldFlags = $env:RUSTFLAGS
$oldEncodedFlags = $env:CARGO_ENCODED_RUSTFLAGS
$oldSource = $env:ARTEX_SOURCE_COMMIT
$cargoHome = if ($env:CARGO_HOME) { [IO.Path]::GetFullPath($env:CARGO_HOME) } else { Join-Path ([Environment]::GetFolderPath('UserProfile')) '.cargo' }
$rustupHome = if ($env:RUSTUP_HOME) { [IO.Path]::GetFullPath($env:RUSTUP_HOME) } else { Join-Path ([Environment]::GetFolderPath('UserProfile')) '.rustup' }
$rustFlags = @('-C','target-feature=+crt-static',"--remap-path-prefix=$repoRoot=/artex-source","--remap-path-prefix=$cargoHome=/cargo","--remap-path-prefix=$rustupHome=/rustup",'-C',"link-arg=-Wl,-Map,$buildLinkMap")
try {
    $env:CARGO_TARGET_DIR = Join-Path $repoRoot 'desktop/target'
    $env:RUSTFLAGS = $null
    # An encoded argument separator preserves paths with spaces without shell parsing.
    $env:CARGO_ENCODED_RUSTFLAGS = [string]::Join([char]31, $rustFlags)
    $env:ARTEX_SOURCE_COMMIT = $sourceCommit
    $cargo = (Get-Command cargo -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
    $build = Start-PackageProcess $cargo @('build','--release','--locked','--manifest-path','desktop/native/Cargo.toml') 'release-build'
    Confirm-PackageProcess $build 'Native release build'
    $binary = Join-Path $package 'ARTEX.exe'
    Copy-Item -LiteralPath (Join-Path $env:CARGO_TARGET_DIR 'release/artex-native.exe') -Destination $binary
    $binaryVersion = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($binary)
    if ($binaryVersion.ProductVersion -ne $Version -or $binaryVersion.FileDescription -ne 'ARTEX Desktop') { throw 'Windows executable version metadata does not match the release' }
} finally {
    $env:CARGO_TARGET_DIR = $oldTarget
    $env:RUSTFLAGS = $oldFlags
    $env:CARGO_ENCODED_RUSTFLAGS = $oldEncodedFlags
    $env:ARTEX_SOURCE_COMMIT = $oldSource
}
if (-not (Test-Path -LiteralPath $buildLinkMap)) { throw 'GNU linker map was not produced' }
Copy-Item -LiteralPath $buildLinkMap -Destination $linkMap
if (-not (Test-Path -LiteralPath $linkMap)) { throw 'GNU linker map was not produced' }
Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE') -Destination (Join-Path $package 'LICENSE.txt')
$python = (Get-Command python -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
$collector = Start-PackageProcess $python @((Join-Path $PSScriptRoot 'collect-licenses.py'),'--output',(Join-Path $package 'licenses'),'--target',$target,'--binary',$binary,'--link-map',$linkMap,'--cache',$LicenseCache) 'collect-licenses'
Confirm-PackageProcess $collector 'License and native runtime collection'
$licenseIndex = Join-Path $package 'licenses/INDEX.json'
$licenseEvidence = Get-Content -LiteralPath $licenseIndex -Raw -Encoding UTF8 | ConvertFrom-Json
if ($licenseEvidence.binary_distribution_ready -ne $true -or $licenseEvidence.status -ne 'verified-distribution-inputs') { throw 'License collector did not verify distribution inputs' }
$licenseCollection = @{passed=$true;index_sha256=(Get-FileHash -LiteralPath $licenseIndex -Algorithm SHA256).Hash.ToLowerInvariant();collector_sha256=(Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'collect-licenses.py') -Algorithm SHA256).Hash.ToLowerInvariant();input_policy_sha256=(Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'license-inputs.json') -Algorithm SHA256).Hash.ToLowerInvariant()}

$workspace = Join-Path $repoRoot "desktop/.test-workspaces/packaged-$runId"
$self = Start-PackageProcess $binary @('--self-test','--workspace',$workspace,'--profile',(Join-Path $audit 'self-test-profile')) 'self-test'
if (-not $self.WaitForExit(60000)) { throw "Packaged self-test exceeded deadline; PID $($self.Id) retained for inspection" }
Confirm-PackageProcess $self 'Packaged self-test'
$profile = Join-Path $audit 'verification-profile'
$capture = Join-Path $audit 'native-window.png'
$smokeRelease = Join-Path $audit 'icon-window-release.ready'
$gui = Start-PackageProcess $binary @('--workspace',$workspace,'--read-only','--profile',$profile,'--smoke','--smoke-dialog','about','--smoke-hold-until',$smokeRelease,'--screenshot',$capture) 'window'
$iconPath = Join-Path $audit 'icon-verification.json'
try {
    $iconCheck = Start-PackageProcess $python @((Join-Path $PSScriptRoot 'verify-icon-resources.py'),'--binary',$binary,'--source-ico',(Join-Path $repoRoot 'desktop/native/assets/branding/artex-app-icon.ico'),'--window-pid',([string]$gui.Id),'--output',$iconPath) 'icons'
    if (-not $iconCheck.WaitForExit(30000)) { throw "Packaged icon verification exceeded deadline; PID $($iconCheck.Id) retained for inspection" }
    Confirm-PackageProcess $iconCheck 'Packaged executable/window icons'
    $iconVerification = Get-Content -LiteralPath $iconPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if (-not $iconVerification.passed -or -not $iconVerification.window) { throw 'Packaged icon evidence incomplete' }
} finally {
    [IO.File]::WriteAllText($smokeRelease, 'release')
}
$children = @()
$modules = @()
$deadline = [DateTime]::UtcNow.AddSeconds(110)
while (-not $gui.HasExited -and [DateTime]::UtcNow -lt $deadline) {
    $children += @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $($gui.Id)" | Select-Object ProcessId,Name,ExecutablePath)
    try { $modules += @((Get-Process -Id $gui.Id -ErrorAction Stop).Modules | ForEach-Object { $_.ModuleName }) } catch {}
    Start-Sleep -Milliseconds 250
    $gui.Refresh()
}
if (-not $gui.HasExited) { throw "Packaged GUI exceeded deadline; PID $($gui.Id) retained for inspection" }
Confirm-PackageProcess $gui 'Packaged GUI'
$window = Get-Content -LiteralPath (Join-Path $profile 'native-window-smoke.json') -Raw -Encoding UTF8 | ConvertFrom-Json
if ($window.error -or -not $window.orderly_shutdown -or -not $window.screenshot_received -or $window.rendered_sections.Count -ne 23 -or -not (Test-Path -LiteralPath $capture)) { throw 'Packaged native window proof incomplete' }
if ($window.about_dialog.open -ne $true -or $window.about_dialog.requested_in_smoke -ne $true -or $window.about_dialog.settled_before_capture -ne $true) { throw 'Packaged legal notice window was not displayed after its animation settled' }
if ($window.window_chrome.integrated_titlebar -ne $true -or $window.window_chrome.requested_native_decorations -ne $false -or $window.window_chrome.close_uses_application_lifecycle -ne $true) { throw 'Packaged custom window frame configuration incomplete' }
$children = @($children | Sort-Object ProcessId -Unique)
$modules = @($modules | Sort-Object -Unique)
if (@($children | Where-Object { $_.Name -match 'artex-engine|msedgewebview2' }).Count -gt 0 -or @($modules | Where-Object { $_ -match 'WebView2Loader|wry' }).Count -gt 0) { throw 'Packaged app launched legacy GUI components' }
$report = @{passed=$true;completed_utc=[DateTime]::UtcNow.ToString('o');version=$Version;source_commit=$sourceCommit;bundle=$package;artifact_root=$artifactRoot;executable=$binary;executable_sha256=(Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant();executable_bytes=(Get-Item -LiteralPath $binary).Length;verification_result=$verificationPath;verification_sha256=(Get-FileHash -LiteralPath $verificationPath -Algorithm SHA256).Hash.ToLowerInvariant();rust_version=$rustVersion;compiler_path_remapping=$true;legal_notice_window_verified=$true;license_collection=$licenseCollection;native_link_map_sha256=(Get-FileHash -LiteralPath $linkMap -Algorithm SHA256).Hash.ToLowerInvariant();platform='Windows x86_64';workspace=$workspace;capture=$capture;self_test_exit_code=$self.ExitCode;window_exit_code=$gui.ExitCode;orderly_shutdown=$window.orderly_shutdown;rendered_sections=$window.rendered_sections;observed_children=$children;observed_modules=$modules;physical_macos_arch_qualification=$false;source_management_actions=$window.source_management_actions}
$report.icon_verification = $iconVerification
$report.icon_verification_sha256 = (Get-FileHash -LiteralPath $iconPath -Algorithm SHA256).Hash.ToLowerInvariant()
$report | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $artifactRoot 'package-result.json') -Encoding UTF8
Write-Output "Packaged native application PASS: $artifactRoot"
