param([switch]$FullRegression, [string]$SharedRegressionReport, [string]$SoftwareOpenGlDirectory)
$ErrorActionPreference = 'Stop'
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..'))
Set-Location -LiteralPath $repoRoot
$env:CARGO_TARGET_DIR = Join-Path $repoRoot 'desktop/target'
$runId = [guid]::NewGuid().ToString('N')
$resultRoot = Join-Path $repoRoot "desktop/native/test-results/$runId"
New-Item -ItemType Directory -Path $resultRoot -Force | Out-Null
$steps = [System.Collections.Generic.List[object]]::new()
function ConvertTo-NativeArgument([string]$value) {
    $escaped = [regex]::Replace($value, '(\\*)"', {
        param($match)
        $match.Groups[1].Value + $match.Groups[1].Value + '\"'
    })
    $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
    return '"' + $escaped + '"'
}
function Start-NativeProcess([string]$file, [string[]]$nativeArguments, [string]$name) {
    $argumentText = ($nativeArguments | ForEach-Object { ConvertTo-NativeArgument $_ }) -join ' '
    $nativeProcess = Start-Process -FilePath $file -ArgumentList $argumentText -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $resultRoot "$name.stdout.log") -RedirectStandardError (Join-Path $resultRoot "$name.stderr.log")
    # Retain the OS handle while the child is alive. Windows PowerShell can
    # otherwise return a Process with no readable exit code after it exits.
    $null = $nativeProcess.Handle
    return $nativeProcess
}
function Read-ExitCode($process, [string]$name) {
    $process.WaitForExit()
    if ($null -eq $process.ExitCode) { throw "$name has no confirmed exit code" }
    return [int]$process.ExitCode
}
function Invoke-NativeCheck([string]$name, [string]$file, [string[]]$nativeArguments) {
    $started = [DateTime]::UtcNow.ToString('o')
    $process = Start-NativeProcess $file $nativeArguments $name
    $exit = Read-ExitCode $process $name
    $log = Join-Path $resultRoot "$name.log"
    # Native stderr is redirected to a file rather than merged with 2>&1:
    # Windows PowerShell can otherwise treat benign warnings as ErrorRecords.
    @('--- stdout ---') + @(Get-Content -LiteralPath (Join-Path $resultRoot "$name.stdout.log")) + @('--- stderr ---') + @(Get-Content -LiteralPath (Join-Path $resultRoot "$name.stderr.log")) | Set-Content -LiteralPath $log -Encoding UTF8
    [void]$steps.Add(@{name=$name;started_utc=$started;finished_utc=[DateTime]::UtcNow.ToString('o');exit_code=$exit;log=$log})
    if ($exit -ne 0) {
        foreach ($stream in @('stdout','stderr')) {
            Write-Host "Failed $name $stream tail:"
            Get-Content -LiteralPath (Join-Path $resultRoot "$name.$stream.log") -Tail 180 | ForEach-Object { Write-Host $_ }
        }
        throw "$name failed with exit $exit. See $log"
    }
    Write-Host "$name PASS: $log"
}
function Invoke-CargoCheck([string]$name, [string[]]$cargoArguments) {
    Invoke-NativeCheck $name (Get-Command cargo -CommandType Application -ErrorAction Stop | Select-Object -First 1 -ExpandProperty Source) $cargoArguments
}
$sharedParent = $null
if ($SharedRegressionReport) {
    $parentPath = (Resolve-Path -LiteralPath $SharedRegressionReport).Path
    $parent = Get-Content -LiteralPath $parentPath -Raw | ConvertFrom-Json
    if (-not $parent.passed -or -not $parent.full_regression) { throw 'Reused shared regression must have a passing full report' }
    # Reuse only the shared workspace suite. Services and host/UI are always
    # executed again, including native-core's platform-specific fixtures.
    $prefixes = @('desktop/crates/','desktop/contracts/') | ForEach-Object { [IO.Path]::GetFullPath((Join-Path $repoRoot $_)) }
    $manifests = @('desktop/Cargo.toml','desktop/Cargo.lock','desktop/native/Cargo.toml','desktop/native/Cargo.lock','desktop/native-core/Cargo.toml','desktop/native-core/Cargo.lock') | ForEach-Object { [IO.Path]::GetFullPath((Join-Path $repoRoot $_)) }
    $checked = 0
    foreach ($source in $parent.source_files) {
        $path = [IO.Path]::GetFullPath($source.path)
        $shared = $manifests -contains $path
        foreach ($prefix in $prefixes) { if ($path.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { $shared = $true } }
        if ($shared) {
            if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $source.sha256) { throw "Shared source changed: $path" }
            $checked++
        }
    }
    $sharedInputs = @(@('desktop/crates','desktop/contracts') | ForEach-Object { Get-ChildItem -LiteralPath (Join-Path $repoRoot $_) -Recurse -File | Where-Object { $_.Extension -in @('.rs','.json','.txt','.dat','.pem','.png','.svg') -or $_.Name -in @('Cargo.toml','Cargo.lock') } })
    foreach ($inputFile in $sharedInputs) {
        if (@($parent.source_files | Where-Object { $_.path -eq $inputFile.FullName }).Count -ne 1) { throw "Shared input is absent from reused evidence: $($inputFile.FullName). Run -FullRegression." }
    }
    if ($checked -lt 100) { throw 'Shared source evidence is incomplete' }
    foreach ($name in @('shared-regression')) {
        $step = @($parent.steps | Where-Object { $_.name -eq $name -and $_.exit_code -eq 0 })
        if ($step.Count -ne 1) { throw "Missing passing $name evidence" }
        $step[0] | Add-Member -NotePropertyName reused_unchanged_sources -NotePropertyValue $true -Force
        [void]$steps.Add($step[0])
    }
    $sharedParent = @{path=$parentPath;sha256=(Get-FileHash -LiteralPath $parentPath -Algorithm SHA256).Hash.ToLowerInvariant();checked_source_count=$checked}
    Write-Host "Shared regression reused after $checked source hash checks"
}
Invoke-CargoCheck 'native-core' @('test','--locked','--manifest-path','desktop/native-core/Cargo.toml')
Invoke-CargoCheck 'native-host' @('test','--locked','--manifest-path','desktop/Cargo.toml','-p','artex-host','--no-default-features')
Invoke-CargoCheck 'native-ui' @('test','--locked','--manifest-path','desktop/native/Cargo.toml')
if ($FullRegression -and -not $sharedParent) { Invoke-CargoCheck 'shared-regression' @('test','--locked','--manifest-path','desktop/Cargo.toml','--workspace') }
Invoke-CargoCheck 'build' @('build','--locked','--manifest-path','desktop/native/Cargo.toml')
$featureLog = Join-Path $resultRoot 'native-features.log'
Invoke-CargoCheck 'native-features' @('tree','--locked','--manifest-path','desktop/native/Cargo.toml','-e','features')
$features = Get-Content -LiteralPath $featureLog -Raw
if ($features -match '(?m)^.*\b(tauri|wry|artex-engine) v' -or $features -match 'artex-host feature "legacy-client"') { throw 'Native application includes a legacy GUI/sidecar dependency' }
$executable = Join-Path $env:CARGO_TARGET_DIR 'debug/artex-native.exe'
$testedSha = (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash.ToLowerInvariant()
$softwareOpenGl = $null
$softwareObserved = @{}
if ($SoftwareOpenGlDirectory) {
    $fixtureRoot = (Resolve-Path -LiteralPath $SoftwareOpenGlDirectory).Path
    $expectedDlls = @{
        'opengl32.dll' = '75b295fc80df0eaa8f1264c28663f6932c8b72d36513eb092556608858b7f5e0'
        'libgallium_wgl.dll' = '53e49adf87590d5b0cab8553b1e98084812040cc1fca8c487695b17b7af3702e'
    }
    $provider = Get-Content -LiteralPath (Join-Path $fixtureRoot 'mesa-provenance.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($provider.version -ne '26.2.4' -or $provider.architecture -ne 'x64' -or $provider.archive_sha256 -ne '351fc8c8b695878ffb3eaa044b3ead08672a48b1a045e3c3e3975811df0f6695') { throw 'CI OpenGL provider identity differs from the pinned publisher archive' }
    $guiRoot = Join-Path $resultRoot 'software-opengl'
    New-Item -ItemType Directory -Path $guiRoot | Out-Null
    Copy-Item -LiteralPath $executable -Destination (Join-Path $guiRoot 'artex-native.exe')
    foreach ($name in $expectedDlls.Keys) {
        $dll = Get-Item -LiteralPath (Join-Path $fixtureRoot $name)
        if ($dll.PSIsContainer -or ($dll.Attributes -band [IO.FileAttributes]::ReparsePoint) -or (Get-FileHash -LiteralPath $dll.FullName -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expectedDlls[$name]) { throw "Unverified CI OpenGL file: $name" }
        Copy-Item -LiteralPath $dll.FullName -Destination (Join-Path $guiRoot $name)
    }
    $executable = Join-Path $guiRoot 'artex-native.exe'
    if ((Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash.ToLowerInvariant() -ne $testedSha) { throw 'Relocated CI executable changed' }
    $env:GALLIUM_DRIVER = 'llvmpipe'
    $env:LIBGL_ALWAYS_SOFTWARE = '1'
    $env:MESA_SHADER_CACHE_DISABLE = 'true'
    $softwareOpenGl = @{version=$provider.version;archive_sha256=$provider.archive_sha256;renderer='llvmpipe';expected_files=$expectedDlls;loaded_files=@{};scope='CI-only app-local software renderer; not hardware-driver qualification'}
}
function Read-WindowModules($primary) {
    try {
        foreach ($module in (Get-Process -Id $primary.Id -ErrorAction Stop).Modules) {
            if ($softwareOpenGl -and $softwareOpenGl.expected_files.ContainsKey($module.ModuleName)) { $softwareObserved[$module.ModuleName] = $module.FileName }
            $module.ModuleName
        }
    } catch {} # A just-closed process can race module enumeration.
}

$workspace = Join-Path $repoRoot "desktop/.test-workspaces/native-$runId"
Invoke-NativeCheck 'integrated-self-test' $executable @('--self-test','--workspace',$workspace)
$profile = Join-Path $resultRoot 'profile'
$capture = Join-Path $resultRoot 'native-window.png'
$smokeRelease = Join-Path $resultRoot 'smoke-release.ready'
$process = Start-NativeProcess $executable @('--workspace',$workspace,'--profile',$profile,'--smoke','--smoke-hold-until',$smokeRelease,'--screenshot',$capture) 'window'
try {
$children = @()
$modules = @()
$descriptorPath = Join-Path $profile 'native-launch/session.info'
$readyDeadline = [DateTime]::UtcNow.AddSeconds(15)
while (-not (Test-Path -LiteralPath $descriptorPath) -and -not $process.HasExited -and [DateTime]::UtcNow -lt $readyDeadline) {
    Start-Sleep -Milliseconds 20
    $process.Refresh()
}
if ($process.HasExited -or -not (Test-Path -LiteralPath $descriptorPath)) { throw 'Primary session unavailable for second-launch testing' }
$sessionBefore = Get-Content -LiteralPath $descriptorPath -Raw
if ($sessionBefore -notmatch '(?m)^session=instance_[0-9a-f]{32}\r?$' -or $sessionBefore -notmatch '(?m)^launch_protocol=2\r?$') { throw 'Primary session descriptor invalid' }
$secondStarted = [DateTime]::UtcNow.ToString('o')
$second = Start-NativeProcess $executable @('--workspace',$workspace,'--profile',$profile,'--read-only') 'second-launch'
$observedMailbox = [System.Collections.Generic.List[object]]::new()
$secondDeadline = [DateTime]::UtcNow.AddSeconds(10)
while (-not $second.HasExited -and [DateTime]::UtcNow -lt $secondDeadline) {
    Get-ChildItem -LiteralPath (Join-Path $profile 'native-launch') -File -Filter 'launch_*.bin' | ForEach-Object {
        try {
            $bytes = [System.IO.File]::ReadAllBytes($_.FullName)
            if ($bytes.Length -gt 8) { [void]$observedMailbox.Add(@{name=$_.Name;read_only_flag=[int]$bytes[8]}) }
        } catch {} # Listener may have consumed it between enumeration and read.
    }
    $second.Refresh()
    $process.Refresh()
    if ($process.HasExited) { throw 'Primary exited while second launcher was running' }
    Start-Sleep -Milliseconds 5
}
if (-not $second.HasExited) { throw "Second launcher did not return; pid $($second.Id) retained for inspection" }
$secondExit = Read-ExitCode $second 'second-launch'
if ($secondExit -ne 0) { throw "Second launch failed: exit $secondExit" }
$drained = $false
$drainDeadline = [DateTime]::UtcNow.AddSeconds(5)
do {
    $process.Refresh()
    if ($process.HasExited) { throw 'Primary exited before mailbox consumption was confirmed' }
    $drained = @(Get-ChildItem -LiteralPath (Join-Path $profile 'native-launch') -File -Filter 'launch_*.bin').Count -eq 0
    if (-not $drained) { Start-Sleep -Milliseconds 25 }
} while (-not $drained -and [DateTime]::UtcNow -lt $drainDeadline)
if (-not $drained -or (Get-Content -LiteralPath $descriptorPath -Raw) -ne $sessionBefore) { throw 'Launch queue did not drain within the same active session' }
$observedMailbox = @($observedMailbox | Sort-Object name -Unique)
if (@($observedMailbox | Where-Object { $_.read_only_flag -ne 1 }).Count -gt 0) { throw 'Observed handoff lost read-only mode' }
$owners = @(Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $executable -and $_.CommandLine -and $_.CommandLine.IndexOf($profile, [StringComparison]::OrdinalIgnoreCase) -ge 0 } | Select-Object ProcessId,Name,ExecutablePath)
if ($owners.Count -ne 1 -or $owners[0].ProcessId -ne $process.Id) { throw 'Second launch created another persistent profile owner' }
$process.Refresh()
if ($process.HasExited) { throw 'Primary exited before the verifier released its smoke hold' }
$handoff = @{started_utc=$secondStarted;finished_utc=[DateTime]::UtcNow.ToString('o');primary_pid=$process.Id;secondary_pid=$second.Id;secondary_exit_code=$secondExit;primary_only=$true;read_only_requested=$true;session_descriptor_sha256=(Get-FileHash -LiteralPath $descriptorPath -Algorithm SHA256).Hash.ToLowerInvariant();observed_mailbox=$observedMailbox;queue_accepted_by_successful_busy_handoff=$true;queue_drained_same_session=$true;gui_request_handling_verified=$false;gui_workspace_switch_verified=$false;read_only_preservation_unit='instance::tests::launch_handoff_is_bounded_session_fenced_and_does_not_open_workspace';evidence_note='Exit 0 while the primary is alive proves BUSY-branch submission. A drained queue in the unchanged session proves listener consumption, not GUI workspace-switch acceptance.'}
$handoff | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $resultRoot 'second-launch-result.json') -Encoding UTF8
# Collect a live sample while the smoke hold guarantees the window is alive.
# Normal smoke may otherwise finish before the first post-release observation.
$children += @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $($process.Id)" | Select-Object ProcessId,Name,ExecutablePath)
$modules += @(Read-WindowModules $process)
$iconVerificationPath = Join-Path $resultRoot 'icon-verification.json'
Invoke-NativeCheck 'icon-resource-window' (Get-Command python -CommandType Application -ErrorAction Stop | Select-Object -First 1 -ExpandProperty Source) @((Join-Path $PSScriptRoot 'verify-icon-resources.py'),'--binary',$executable,'--source-ico',(Join-Path $repoRoot 'desktop/native/assets/branding/artex-app-icon.ico'),'--window-pid',([string]$process.Id),'--output',$iconVerificationPath)
$iconVerification = Get-Content -LiteralPath $iconVerificationPath -Raw -Encoding UTF8 | ConvertFrom-Json
if (-not $iconVerification.passed -or $iconVerification.binary_sha256 -ne $testedSha -or -not $iconVerification.window) { throw 'Live native window or executable icon verification failed' }
# Release only after the real BUSY handoff returned 0, its same-session queue
# drained, and the primary is still the sole live profile owner. Capture/close
# modes without this diagnostic flag keep their existing automatic behavior.
$releaseTemp = Join-Path $resultRoot 'smoke-release.tmp'
[System.IO.File]::WriteAllText($releaseTemp, 'release')
Move-Item -LiteralPath $releaseTemp -Destination $smokeRelease
$deadline = [DateTime]::UtcNow.AddSeconds(110)
while (-not $process.HasExited -and [DateTime]::UtcNow -lt $deadline) {
    $children += @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $($process.Id)" | Select-Object ProcessId,Name,ExecutablePath)
    $modules += @(Read-WindowModules $process)
    Start-Sleep -Milliseconds 250
    $process.Refresh()
}
if (-not $process.HasExited) { throw "Native GUI did not exit normally; pid $($process.Id) retained for inspection" }
$windowExit = Read-ExitCode $process 'window'
if ($windowExit -ne 0) { throw "Native GUI failed: exit $windowExit" }
$window = Get-Content -LiteralPath (Join-Path $profile 'native-window-smoke.json') -Raw | ConvertFrom-Json
if (-not $window.orderly_shutdown -or -not $window.screenshot_received -or $window.error -or $window.rendered_sections.Count -ne 23) { throw 'Native window proof incomplete' }
if ($window.smoke_hold_handshake.requested -ne $true -or $window.smoke_hold_handshake.released -ne $true) { throw 'Native window did not retain confirmed smoke hold/release evidence' }
if ($window.window_chrome.integrated_titlebar -ne $true -or $window.window_chrome.requested_native_decorations -ne $false -or $window.window_chrome.close_uses_application_lifecycle -ne $true) { throw 'Custom window frame configuration incomplete' }
$received = @($window.received_launch_requests | Where-Object { $_.read_only -eq $true -and $_.workspace -and ([System.IO.Path]::GetFullPath($_.workspace) -replace '^\\\\\?\\','') -eq [System.IO.Path]::GetFullPath($workspace) })
if ($received.Count -ne 1 -or $received[0].request_id -notmatch '^launch_[0-9a-f]{32}$') { throw 'GUI did not record the exact read-only second-launch request' }
if ($observedMailbox.Count -gt 0 -and @($observedMailbox | Where-Object { $_.name -eq ($received[0].request_id + '.bin') }).Count -ne 1) { throw 'GUI launch ID does not match observed mailbox publication' }
$handoff.gui_request_handling_verified = $true
$handoff.smoke_hold_release_verified = $true
$handoff.received_request = $received[0]
$handoff.evidence_note = 'Exit 0 while primary alive, stable-session drained queue, and GUI received request ID/path/read_only jointly verify handoff. Actual user acceptance of the mode-switch dialog is not automated.'
$handoff | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $resultRoot 'second-launch-result.json') -Encoding UTF8
if (-not (Test-Path -LiteralPath $capture)) { throw 'Native screenshot missing' }
$children = @($children | Sort-Object ProcessId -Unique)
$modules = @($modules | Sort-Object -Unique)
if ($softwareOpenGl) {
    foreach ($name in $softwareOpenGl.expected_files.Keys) {
        if (-not $softwareObserved.ContainsKey($name) -or -not [StringComparer]::OrdinalIgnoreCase.Equals([IO.Path]::GetFullPath($softwareObserved[$name]), [IO.Path]::GetFullPath((Join-Path $guiRoot $name)))) { throw "CI window did not load its verified app-local OpenGL file: $name" }
        $loadedSha = (Get-FileHash -LiteralPath $softwareObserved[$name] -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($loadedSha -ne $softwareOpenGl.expected_files[$name]) { throw "Loaded CI OpenGL file changed: $name" }
        $softwareOpenGl.loaded_files[$name] = @{path=$softwareObserved[$name];sha256=$loadedSha}
    }
    $softwareOpenGl.loaded_files_verified = $true
}
if (@($children | Where-Object { $_.Name -match 'artex-engine|msedgewebview2' }).Count -gt 0 -or @($modules | Where-Object { $_ -match 'WebView2Loader|wry' }).Count -gt 0) { throw 'Native window launched a legacy sidecar/WebView' }
if ((Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash.ToLowerInvariant() -ne $testedSha) { throw 'Executable changed during verification' }
$sourcePaths = [System.Collections.Generic.List[string]]::new()
foreach ($directory in @('desktop/native/src','desktop/native/examples','desktop/native-core/src','desktop/crates')) {
    Get-ChildItem -LiteralPath (Join-Path $repoRoot $directory) -Recurse -File | Where-Object { $_.Extension -in @('.rs','.json','.txt','.dat','.pem','.png','.svg') -or $_.Name -in @('Cargo.toml','Cargo.lock') } | ForEach-Object { [void]$sourcePaths.Add($_.FullName) }
}
foreach ($manifest in @('desktop/Cargo.toml','desktop/Cargo.lock','desktop/native/Cargo.toml','desktop/native/Cargo.lock','desktop/native-core/Cargo.toml','desktop/native-core/Cargo.lock')) {
    $path = Join-Path $repoRoot $manifest
    if (Test-Path -LiteralPath $path) { [void]$sourcePaths.Add($path) }
}
foreach ($directory in @('desktop/contracts','desktop/native/assets/fonts','desktop/native/assets/branding','desktop/native/scripts')) {
    Get-ChildItem -LiteralPath (Join-Path $repoRoot $directory) -Recurse -File | Where-Object { $_.FullName -notmatch '[\\/]__pycache__[\\/]' } | ForEach-Object { [void]$sourcePaths.Add($_.FullName) }
}
# The About dialog embeds the root AGPL text. Bind it to the verified source.
[void]$sourcePaths.Add((Join-Path $repoRoot 'LICENSE'))
[void]$sourcePaths.Add((Join-Path $repoRoot 'desktop/native/build.rs'))
$sourceFiles = @($sourcePaths | Sort-Object -Unique | ForEach-Object { @{path=$_;sha256=(Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLowerInvariant()} })
$report = @{passed=$true;completed_utc=[DateTime]::UtcNow.ToString('o');executable=$executable;executable_sha256=$testedSha;workspace=$workspace;capture=$capture;rendered_sections=$window.rendered_sections;source_management_actions=$window.source_management_actions;orderly_shutdown=$window.orderly_shutdown;window_exit_code=$windowExit;second_launch=$handoff;observed_child_processes=$children;observed_modules=$modules;source_files=$sourceFiles;steps=$steps.ToArray();full_regression=([bool]$FullRegression -or $null -ne $sharedParent);shared_regression_parent=$sharedParent;software_opengl=$softwareOpenGl;platform='Windows x86_64'}
$report.icon_verification = $iconVerification
$report | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $resultRoot 'verification-result.json') -Encoding UTF8
Write-Output "Native verification PASS: $resultRoot"
} catch {
    # Only these isolated validation logs are printed. Never upload a profile,
    # its preferences, vault references, launch inbox or workspace contents.
    $process.Refresh()
    $primaryExit = if ($process.HasExited) { Read-ExitCode $process 'window diagnostics' } else { 'still-running' }
    Write-Host "Native window failure diagnostics: primary_pid=$($process.Id) confirmed_exit_code=$primaryExit"
    foreach ($name in @('window.stdout.log','window.stderr.log')) {
        $logPath=Join-Path $resultRoot $name
        if (Test-Path -LiteralPath $logPath) { Write-Host "--- $name ---"; Get-Content -LiteralPath $logPath -Tail 120 | ForEach-Object { Write-Host $_ } }
    }
    $smokePath=Join-Path $profile 'native-window-smoke.json'
    if (Test-Path -LiteralPath $smokePath) {
        try {
            $smokeProof=Get-Content -LiteralPath $smokePath -Raw | ConvertFrom-Json
            Write-Host (@{screenshot_received=$smokeProof.screenshot_received;orderly_shutdown=$smokeProof.orderly_shutdown;rendered_section_count=$smokeProof.rendered_sections.Count;received_launch_count=$smokeProof.received_launch_requests.Count;smoke_hold=$smokeProof.smoke_hold_handshake;error_code=$smokeProof.error.code} | ConvertTo-Json -Compress -Depth 4)
        } catch { Write-Host 'Native smoke report is present but could not be parsed' }
    } else { Write-Host 'Native smoke report is absent' }
    throw
}
