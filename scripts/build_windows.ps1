param(
    [switch]$SkipInstaller
)

$ErrorActionPreference = "Stop"

$ProjectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Version = "2.2.0"
$AppDirName = "Google-Scholar-Scraper-v$Version"
$PortableName = "Google-Scholar-Scraper-v$Version-Portable-Windows-x64.zip"
$InstallerName = "Google-Scholar-Scraper-v$Version-Setup-Windows-x64.exe"
$BuildDir = Join-Path $ProjectRoot "build"
$DistDir = Join-Path $ProjectRoot "dist"
$ReleaseDir = Join-Path $DistDir "release"
$PortablePath = Join-Path $ReleaseDir $PortableName
$ChecksumPath = Join-Path $ReleaseDir "SHA256SUMS.txt"
$SpecPath = Join-Path $ProjectRoot "packaging\pyinstaller\GoogleScholarScraper.spec"
$InstallerScript = Join-Path $ProjectRoot "installer\GoogleScholarScraper.iss"
$ConstraintsPath = Join-Path $ProjectRoot "constraints\release-2.2.0.txt"
$LicenseCollectorPath = Join-Path $ProjectRoot "scripts\collect_third_party_licenses.py"

function Remove-GeneratedPath {
    param([Parameter(Mandatory=$true)][string]$Path)
    $root = [System.IO.Path]::GetFullPath($ProjectRoot)
    $target = [System.IO.Path]::GetFullPath($Path)
    if (-not $target.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove path outside repository: $target"
    }
    if ((Split-Path -Leaf $target) -notin @("build", "dist")) {
        throw "Refusing to remove unexpected generated path: $target"
    }
    if (Test-Path -LiteralPath $target) {
        Remove-Item -LiteralPath $target -Recurse -Force
    }
}

function Get-InnoCompiler {
    $fromPath = Get-Command "ISCC.exe" -ErrorAction SilentlyContinue
    if ($fromPath) {
        return $fromPath.Source
    }

    $candidates = @(
        "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
        "C:\Program Files\Inno Setup 6\ISCC.exe"
    )
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate) {
            return $candidate
        }
    }
    return $null
}

function Get-Sha256 {
    param([Parameter(Mandatory=$true)][string]$Path)
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $sha = [System.Security.Cryptography.SHA256]::Create()
        try {
            $hash = $sha.ComputeHash($stream)
            return -join ($hash | ForEach-Object { $_.ToString("x2") })
        } finally {
            $sha.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

function Copy-DistributionNotices {
    param([Parameter(Mandatory=$true)][string]$AppDir)
    foreach ($name in @("LICENSE", "NOTICE", "COMMERCIAL_LICENSE.md", "THIRD_PARTY_NOTICES.txt")) {
        $source = Join-Path $ProjectRoot $name
        $target = Join-Path $AppDir $name
        if (-not (Test-Path -LiteralPath $source)) {
            throw "Required distribution notice is missing from source: $source"
        }
        Copy-Item -LiteralPath $source -Destination $target -Force
    }
}

function Copy-UserDocumentation {
    param([Parameter(Mandatory=$true)][string]$AppDir)
    $readme = Join-Path $ProjectRoot "README.md"
    if (-not (Test-Path -LiteralPath $readme)) {
        throw "Required user documentation is missing from source: $readme"
    }
    Copy-Item -LiteralPath $readme -Destination (Join-Path $AppDir "README.md") -Force

    $docsSource = Join-Path $ProjectRoot "docs"
    $docsTarget = Join-Path $AppDir "docs"
    New-Item -ItemType Directory -Path $docsTarget -Force | Out-Null

    $importGuide = Join-Path $docsSource "ranking-imports.md"
    if (-not (Test-Path -LiteralPath $importGuide)) {
        throw "Required ranking import guide is missing from source: $importGuide"
    }
    Copy-Item -LiteralPath $importGuide -Destination (Join-Path $docsTarget "ranking-imports.md") -Force

    $templatesSource = Join-Path $docsSource "ranking_templates"
    $templates = @()
    if (Test-Path -LiteralPath $templatesSource) {
        $templates = @(Get-ChildItem -LiteralPath $templatesSource -File -Filter "*.csv")
    }
    if ($templates.Count -eq 0) {
        throw "No header-only ranking CSV templates found in: $templatesSource"
    }
    $templatesTarget = Join-Path $docsTarget "ranking_templates"
    New-Item -ItemType Directory -Path $templatesTarget -Force | Out-Null
    foreach ($template in $templates) {
        Copy-Item -LiteralPath $template.FullName -Destination (Join-Path $templatesTarget $template.Name) -Force
    }
}

function Remove-OptionalHttp2Metadata {
    param([Parameter(Mandatory=$true)][string]$AppDir)
    $internalDir = Join-Path $AppDir "_internal"
    if (-not (Test-Path -LiteralPath $internalDir)) {
        return
    }
    foreach ($pattern in @("h2-*.dist-info", "hpack-*.dist-info", "hyperframe-*.dist-info")) {
        Get-ChildItem -LiteralPath $internalDir -Directory -Filter $pattern -ErrorAction SilentlyContinue |
            Remove-Item -Recurse -Force
    }
}

function Collect-ThirdPartyLicenses {
    param([Parameter(Mandatory=$true)][string]$AppDir)
    $outputDir = Join-Path $AppDir "third_party_licenses"
    python $LicenseCollectorPath `
        --constraints $ConstraintsPath `
        --app-dir $AppDir `
        --output-dir $outputDir
    if ($LASTEXITCODE -ne 0) {
        throw "Third-party license collection failed with exit code $LASTEXITCODE."
    }
}

function Verify-DistributionCompliance {
    param([Parameter(Mandatory=$true)][string]$AppDir)
    foreach ($name in @("LICENSE", "NOTICE", "COMMERCIAL_LICENSE.md", "THIRD_PARTY_NOTICES.txt")) {
        $path = Join-Path $AppDir $name
        if (-not (Test-Path -LiteralPath $path)) {
            throw "Required distribution notice missing: $path"
        }
    }

    $licenseRoot = Join-Path $AppDir "third_party_licenses"
    $requiredLicenseEntries = @(
        "MANIFEST.txt",
        "beautifulsoup4",
        "soupsieve",
        "openpyxl",
        "et-xmlfile",
        "requests",
        "urllib3",
        "charset-normalizer",
        "idna",
        "certifi",
        "typing-extensions",
        "pyinstaller",
        "pyinstaller-hooks-contrib",
        "python",
        "tcl-tk"
    )
    foreach ($entry in $requiredLicenseEntries) {
        $path = Join-Path $licenseRoot $entry
        if (-not (Test-Path -LiteralPath $path)) {
            throw "Required third-party license bundle entry missing: $path"
        }
    }

    foreach ($relative in @("README.md", "docs\ranking-imports.md")) {
        $path = Join-Path $AppDir $relative
        if (-not (Test-Path -LiteralPath $path)) {
            throw "Required user documentation missing from distribution: $path"
        }
    }
    $templates = @(Get-ChildItem -LiteralPath (Join-Path $AppDir "docs\ranking_templates") -File -Filter "*.csv" -ErrorAction SilentlyContinue)
    if ($templates.Count -eq 0) {
        throw "Distribution ships no ranking import templates."
    }
}

Set-Location $ProjectRoot
Remove-GeneratedPath $BuildDir
Remove-GeneratedPath $DistDir
New-Item -ItemType Directory -Path $ReleaseDir -Force | Out-Null

python -m PyInstaller $SpecPath --noconfirm --clean
if ($LASTEXITCODE -ne 0) {
    throw "PyInstaller failed with exit code $LASTEXITCODE."
}

$ExePath = Join-Path $DistDir "$AppDirName\GoogleScholarScraper.exe"
if (-not (Test-Path -LiteralPath $ExePath)) {
    throw "Expected executable was not created: $ExePath"
}
$AppDir = Join-Path $DistDir $AppDirName
Remove-OptionalHttp2Metadata $AppDir
Copy-DistributionNotices $AppDir
Copy-UserDocumentation $AppDir
Collect-ThirdPartyLicenses $AppDir
Verify-DistributionCompliance $AppDir

Compress-Archive -Path $AppDir -DestinationPath $PortablePath -Force
if (-not (Test-Path -LiteralPath $PortablePath)) {
    throw "Expected portable archive was not created: $PortablePath"
}

$checksums = @()
$portableHash = Get-Sha256 $PortablePath
$checksums += "$portableHash  $PortableName"

$InnoCompiler = Get-InnoCompiler
if ($InnoCompiler -and -not $SkipInstaller) {
    & $InnoCompiler $InstallerScript
    if ($LASTEXITCODE -ne 0) {
        throw "Inno Setup compilation failed with exit code $LASTEXITCODE."
    }
    $InstallerPath = Join-Path $DistDir "installer\$InstallerName"
    if (-not (Test-Path -LiteralPath $InstallerPath)) {
        throw "Expected installer was not created: $InstallerPath"
    }
    $installerHash = Get-Sha256 $InstallerPath
    $checksums += "$installerHash  $InstallerName"
} else {
    Write-Host "Inno Setup compiler not found or installer build skipped; installer definition was not compiled."
}

$checksums | Set-Content -LiteralPath $ChecksumPath -Encoding ASCII
Write-Host "Portable archive: $PortablePath"
Write-Host "Checksums: $ChecksumPath"
