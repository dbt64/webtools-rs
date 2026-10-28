param([string]$OutputDirectory)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $repoRoot ("release\native-rust-preview-" + (Get-Date -Format 'yyyyMMdd-HHmmss'))
}
$outputRoot = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $outputRoot) { throw "Preview output already exists: $outputRoot" }
$stage = Join-Path $outputRoot 'stage'
$managerBuild = Join-Path $outputRoot 'manager-build'
$managerStage = Join-Path $stage 'Manager'
New-Item -ItemType Directory -Path $stage, $managerStage -Force | Out-Null

function Invoke-Checked([string]$FilePath, [string[]]$Arguments) {
    Push-Location $repoRoot
    try {
        & $FilePath @Arguments
        if ($LASTEXITCODE -ne 0) { throw "$FilePath failed with exit code $LASTEXITCODE" }
    } finally { Pop-Location }
}

$cargo = (Get-Command cargo.exe -ErrorAction Stop).Source
$node = (Get-Command node.exe -ErrorAction Stop).Source
$pnpm = (Get-Command pnpm.cmd -ErrorAction Stop).Source
$builder = Join-Path $repoRoot 'node_modules\electron-builder\cli.js'
if (-not (Test-Path -LiteralPath $builder)) { throw 'Install the project dependencies before building the preview.' }

Invoke-Checked $cargo @('build', '--locked', '--release', '--features', 'production', '--manifest-path', 'Cargo.toml', '-p', 'slint-launcher')
Invoke-Checked $pnpm @('run', 'build')
Invoke-Checked $node @($builder, '--win', '--x64', '--dir', "--config.directories.output=$managerBuild")

$target = if ($env:CARGO_TARGET_DIR) {
    if ([IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) { $env:CARGO_TARGET_DIR }
    else { Join-Path $repoRoot $env:CARGO_TARGET_DIR }
} else { Join-Path $repoRoot 'target' }
$hostExe = Join-Path $target 'release\slint-launcher.exe'
$manager = Join-Path $managerBuild 'win-unpacked'
foreach ($required in @($hostExe, (Join-Path $manager 'WebTools.exe'), (Join-Path $manager 'resources\app.asar'))) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) { throw "Required preview file is missing: $required" }
}
Copy-Item -LiteralPath $hostExe -Destination (Join-Path $stage 'WebTools.NativeHost.Rust.exe')
Copy-Item -Path (Join-Path $manager '*') -Destination $managerStage -Recurse
New-Item -ItemType Directory -Path (Join-Path $stage 'tools') -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $repoRoot 'resources\everything') -Destination (Join-Path $stage 'tools') -Recurse
Copy-Item -LiteralPath (Join-Path $repoRoot 'resources\app.ico') -Destination (Join-Path $stage 'app.ico')

$notes = @'
WebTools Rust 宿主便携预览

运行 WebTools.NativeHost.Rust.exe。此版本使用当前 Windows 用户的 Nook 数据，启动前请通过托盘退出正在运行的 WPF WebTools。
本包包含 Rust 宿主和 Electron Manager；安装器与自动更新接线尚未包含在此便携包中。
'@
[IO.File]::WriteAllText((Join-Path $stage '预览说明.txt'), $notes, [Text.UTF8Encoding]::new($false))
$zip = Join-Path $outputRoot 'WebTools-Rust-Integration-Preview.zip'
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal
Write-Output "Rust preview ZIP: $zip"
Write-Output "Rust preview executable: $(Join-Path $stage 'WebTools.NativeHost.Rust.exe')"
