<#
  Microsoft Store 提出用の MSIX を作る（署名はしない。Store が再署名する）。

  使い方:
    pwsh -File packaging/windows/make-msix.ps1 `
      -Version 0.3.0 `
      -SourceDir target/release `
      -Output ligrol-0.3.0-x86_64-windows.msix `
      -Publisher "CN=<Partner Center の Publisher>"

  SourceDir には ligrol.exe と pdfium.dll が必要。LICENSE / サードパーティの
  ライセンス全文があれば同梱する。

  ローカルでサイドロードして試すときは、この MSIX に証明書で署名し、
  証明書を LocalMachine\TrustedPeople に入れる必要がある（管理者）。
#>
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$SourceDir,
    [Parameter(Mandatory = $true)][string]$Output,
    [string]$Publisher = "CN=MegaBlackLabel",
    [string]$IdentityName = "MegaBlackLabel.Ligrol"
)

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

$version = $Version.TrimStart('v')
if ($version -notmatch '^\d+\.\d+\.\d+') {
    throw "MSIX のバージョンは semver（例 0.3.0）で指定してください: $Version"
}
# MSIX は 4 桁必要（0.3.0 -> 0.3.0.0）
if ($version -notmatch '^\d+\.\d+\.\d+\.\d+$') {
    $version = "$version.0"
}

$exe = Join-Path $SourceDir 'ligrol.exe'
$dll = Join-Path $SourceDir 'pdfium.dll'
foreach ($required in @($exe, $dll)) {
    if (-not (Test-Path $required)) { throw "必須ファイルが無い: $required" }
}

$stage = Join-Path $env:TEMP ("ligrol-msix-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path (Join-Path $stage 'Assets') -Force | Out-Null
try {
    Copy-Item $exe, $dll $stage
    Copy-Item (Join-Path $here 'Assets\*.png') (Join-Path $stage 'Assets')
    foreach ($name in @('LICENSE', 'lucide-LICENSE.txt', 'pdfium-LICENSE.txt', 'はじめにお読みください.txt')) {
        $candidate = Join-Path $SourceDir $name
        if (Test-Path $candidate) { Copy-Item $candidate $stage }
    }

    $template = Get-Content (Join-Path $here 'AppxManifest.xml.in') -Raw -Encoding UTF8
    $manifest = $template.Replace('@VERSION@', $version).Replace('@PUBLISHER@', $Publisher).Replace('@IDENTITY_NAME@', $IdentityName)
    Set-Content -Path (Join-Path $stage 'AppxManifest.xml') -Value $manifest -Encoding UTF8

    # makeappx は Windows SDK に入っている。最新の x64 を使う。
    $makeappx = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" -Recurse -Filter makeappx.exe -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\' } |
        Sort-Object FullName -Descending |
        Select-Object -First 1
    if (-not $makeappx) { throw 'makeappx.exe が見つかりません（Windows SDK が必要）' }

    & $makeappx.FullName pack /d $stage /p $Output /o
    if ($LASTEXITCODE -ne 0) { throw "makeappx が失敗しました: exit $LASTEXITCODE" }
    Write-Output "MSIX を作成しました: $Output ($version, Publisher=$Publisher)"
}
finally {
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
}
