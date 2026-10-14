param(
    [Parameter(Mandatory=$true)][string]$Destination,
    [string]$ArchivePath
)
$ErrorActionPreference = 'Stop'
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'This CI graphics fixture requires Windows' }
if ($Destination -notmatch '^(?:[A-Za-z]:[\\/]|\\\\[^\\/]+[\\/][^\\/]+[\\/])') { throw 'Use a new absolute CI fixture directory' }
$destinationRoot = [IO.Path]::GetFullPath($Destination)
if (Test-Path -LiteralPath $destinationRoot) { throw 'Preserve existing directories; use a new CI fixture directory' }
# Application-local deployment only. No installer, registry or system DLL changes.
# Publisher archive and the two x64 files are independently pinned.
$version = '26.2.4'
$archiveSha = '351fc8c8b695878ffb3eaa044b3ead08672a48b1a045e3c3e3975811df0f6695'
$sourceUrl = 'https://github.com/pal1000/mesa-dist-win/releases/download/26.2.4/mesa3d-26.2.4-release-msvc.7z'
$expected = @{
    'opengl32.dll' = '75b295fc80df0eaa8f1264c28663f6932c8b72d36513eb092556608858b7f5e0'
    'libgallium_wgl.dll' = '53e49adf87590d5b0cab8553b1e98084812040cc1fca8c487695b17b7af3702e'
}
$work = Join-Path ([IO.Path]::GetTempPath()) ('artex-ci-opengl-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
if ($ArchivePath) {
    $archive = (Resolve-Path -LiteralPath $ArchivePath).Path
} else {
    $archive = Join-Path $work 'mesa3d-26.2.4-release-msvc.7z'
    Invoke-WebRequest -Uri $sourceUrl -OutFile $archive
}
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $archiveSha) { throw 'Mesa publisher archive SHA256 mismatch' }
$seven = Get-Command 7z -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Source
if (-not $seven) { $seven = Join-Path $env:ProgramFiles '7-Zip/7z.exe' }
if (-not (Test-Path -LiteralPath $seven)) { throw 'The CI image must provide 7-Zip' }
$unpacked = Join-Path $work 'x64'
New-Item -ItemType Directory -Path $unpacked | Out-Null
$extractLog = & $seven e $archive "-o$unpacked" '-y' 'x64/opengl32.dll' 'x64/libgallium_wgl.dll' 2>&1
if ($LASTEXITCODE -ne 0) { throw 'Pinned Mesa extraction failed' }
foreach ($name in $expected.Keys) {
    $file = Get-Item -LiteralPath (Join-Path $unpacked $name)
    if ($file.PSIsContainer -or ($file.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Mesa fixture input must be a regular file' }
    if ((Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected[$name]) { throw "Mesa x64 file SHA256 mismatch: $name" }
}
New-Item -ItemType Directory -Path $destinationRoot | Out-Null
foreach ($name in $expected.Keys) { Copy-Item -LiteralPath (Join-Path $unpacked $name) -Destination (Join-Path $destinationRoot $name) }
$proof = @{version=$version;archive_sha256=$archiveSha;source_url=$sourceUrl;architecture='x64';renderer='llvmpipe';scope='CI-only app-local fixture; excluded from release packages';files=$expected}
$proof | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $destinationRoot 'mesa-provenance.json') -Encoding UTF8
Write-Output "Pinned Mesa $version x64 CI fixture prepared"
