<#
  Microsoft Store 提出用の MSIX を作る（署名はしない。Store が再署名する）。

  使い方:
    pwsh -File packaging/windows/make-msix.ps1 `
      -Version 0.3.0 `
      -SourceDir target/release `
      -Output ligrol-0.3.0-x86_64-windows.msix

  Publisher / IdentityName の既定値は Partner Center の「Product identity」の
  値。別のアプリとして出すときだけ上書きする。

  SourceDir には ligrol.exe と pdfium.dll が必要。LICENSE / サードパーティの
  ライセンス全文があれば同梱する。

  ローカルでサイドロードして試すときは、この MSIX に証明書で署名し、
  証明書を LocalMachine\TrustedPeople に入れる必要がある（管理者）。
#>
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$SourceDir,
    [Parameter(Mandatory = $true)][string]$Output,
    [string]$Publisher = "CN=78BBC394-714D-417A-A7D4-36B725971D65",
    [string]$IdentityName = "63355MegaBlackLabel.Ligrol"
)

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

$version = $Version.TrimStart('v')
# 数値の 3 桁 or 4 桁だけを受け付ける（`0.3.0-rc1` などを弾く）。
if ($version -notmatch '^\d+\.\d+\.\d+(\.\d+)?$') {
    throw "MSIX のバージョンは数値の semver（例 0.3.0 / 0.3.0.0）で指定してください: $Version"
}
# MSIX は 4 桁必要（0.3.0 -> 0.3.0.0）
if ($version -notmatch '^\d+\.\d+\.\d+\.\d+$') {
    $version = "$version.0"
}
# Microsoft Store は 4 桁目（リビジョン）に 0 以外を許さない。
# 0.3.0.1 のような指定はパッケージ検証で弾かれる。
if ($version -match '^\d+\.\d+\.\d+\.(?!0+$)\d+$') {
    throw "Microsoft Store はリビジョン（4 桁目）に 0 以外を指定できません: $version（例 0.3.1）"
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
    foreach ($name in @('AI_GUIDE.md', 'LICENSE', 'lucide-LICENSE.txt', 'pdfium-LICENSE.txt', 'はじめにお読みください.txt')) {
        $candidate = Join-Path $SourceDir $name
        if (Test-Path $candidate) { Copy-Item $candidate $stage }
    }

    $template = Get-Content (Join-Path $here 'AppxManifest.xml.in') -Raw -Encoding UTF8
    $manifest = $template.Replace('@VERSION@', $version).Replace('@PUBLISHER@', $Publisher).Replace('@IDENTITY_NAME@', $IdentityName)
    Set-Content -Path (Join-Path $stage 'AppxManifest.xml') -Value $manifest -Encoding UTF8

    # makeappx / makepri は Windows SDK に入っている。最新の x64 を使う。
    $sdkBin = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" -Recurse -Filter makeappx.exe -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\' } |
        Sort-Object FullName -Descending |
        Select-Object -First 1
    if (-not $sdkBin) { throw 'makeappx.exe が見つかりません（Windows SDK が必要）' }

    # 視覚アセットは targetsize / scale 修飾付きの名前で解決されるが、それは
    # PRI（Package Resource Index）に入っていないと効かない。無いとマニフェストに
    # 書いた基本ファイル（44x44）だけが使われ、スタートの一覧で潰れる。
    # 参照: desktop-to-uwp-manual-conversion「Generate a Package Resource Index (PRI) file」
    $makepri = Get-ChildItem $sdkBin.Directory.FullName -Filter makepri.exe -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if (-not $makepri) { throw 'makepri.exe が見つかりません（Windows SDK が必要）' }

    $priConfig = Join-Path $env:TEMP ("ligrol-priconfig-" + [guid]::NewGuid().ToString('N') + ".xml")
    try {
        & $makepri.FullName createconfig /cf $priConfig /dq en-US /o | Out-Null
        & $makepri.FullName new /pr $stage /cf $priConfig /of (Join-Path $stage 'resources.pri') /in $IdentityName /o | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "makepri が失敗しました: exit $LASTEXITCODE" }
    }
    finally {
        Remove-Item $priConfig -Force -ErrorAction SilentlyContinue
    }

    & $sdkBin.FullName pack /d $stage /p $Output /o
    if ($LASTEXITCODE -ne 0) { throw "makeappx が失敗しました: exit $LASTEXITCODE" }
    Write-Output "MSIX を作成しました: $Output ($version, Publisher=$Publisher)"
}
finally {
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
}
