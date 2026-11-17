#requires -Version 5.1
param(
    [ValidateSet('check', 'release', 'deploy')]
    [string]$Mode = 'check',

    [string]$From
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0
$Target = 'x86_64-pc-windows-msvc'
$VersionPattern = '[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?(?:\+[A-Za-z0-9.-]+)?'

function Invoke-Native($Program) {
    & $Program @args
    if ($LASTEXITCODE -ne 0) {
        throw "$Program returned exit code $LASTEXITCODE."
    }
}

function Get-ExternalPath($Path, $Default, $Variable) {
    if (!$Path) {
        $Path = $Default
    }
    if ($Path -notmatch '^(?:[A-Za-z]:[\\/]|\\\\[^\\]+\\[^\\]+)') {
        throw "Use an absolute path for $Variable."
    }
    $fullPath = [IO.Path]::GetFullPath($Path)
    $checkout = $PSScriptRoot.TrimEnd('\')
    if ($fullPath.TrimEnd('\') -eq $checkout -or
        $fullPath.StartsWith("$checkout\", [StringComparison]::OrdinalIgnoreCase)) {
        throw "Set $Variable to a directory outside the source checkout."
    }
    return $fullPath
}

function Get-RepositoryKey {
    $common = Invoke-Native git.exe rev-parse --path-format=absolute --git-common-dir
    $path = [IO.Path]::GetFullPath($common).ToUpperInvariant()
    $sha256 = [Security.Cryptography.SHA256]::Create()
    try {
        $hash = $sha256.ComputeHash([Text.Encoding]::UTF8.GetBytes($path))
        return [BitConverter]::ToString($hash).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha256.Dispose()
    }
}

function New-Package($JobDirectory, $Job, $Commit) {
    # Build the selected commit in a directory outside the checkout.
    $work = Join-Path $JobDirectory 'work'
    $source = Join-Path $work 'source'
    $archive = Join-Path $work 'source.zip'
    $package = Join-Path $JobDirectory 'package'
    $previousTarget = $env:CARGO_TARGET_DIR
    [IO.Directory]::CreateDirectory($work) | Out-Null
    try {
        Invoke-Native git.exe archive --format=zip "--output=$archive" $Commit
        Expand-Archive -LiteralPath $archive -DestinationPath $source
        Push-Location -LiteralPath $source
        try {
            $toolchainFile = [IO.File]::ReadAllText((Join-Path $source 'rust-toolchain.toml'))
            if ($toolchainFile -notmatch '(?m)^channel\s*=\s*"([A-Za-z0-9._-]+)"\s*$') {
                throw 'Set a Rust channel in rust-toolchain.toml.'
            }
            $toolchain = $Matches[1]
            $compiler = Invoke-Native rustup.exe run $toolchain rustc -vV
            $compiler | Write-Host
            if ($compiler -notcontains "host: $Target") {
                throw "Use the native $Target Rust toolchain."
            }
            $env:CARGO_TARGET_DIR = Join-Path $work 'target'
            Invoke-Native rustup.exe run $toolchain cargo fmt --all --check
            Invoke-Native rustup.exe run $toolchain cargo clippy --locked --offline --release --all-targets `
                --target $Target '--' -D warnings
            Invoke-Native rustup.exe run $toolchain cargo build --locked --offline --release --target $Target

            $cargo = [IO.File]::ReadAllText((Join-Path $source 'Cargo.toml'))
            if ($cargo -notmatch ('(?m)^version\s*=\s*"(' + $VersionPattern + ')"\s*$')) {
                throw 'Set a release version in Cargo.toml.'
            }
            $version = $Matches[1]
            $binary = Join-Path $env:CARGO_TARGET_DIR "$Target\release\action.exe"
            if ((Invoke-Native $binary --version) -cne "action $version") {
                throw "The binary must report version $version."
            }

            $name = "action-$version-$Target.exe"
            [IO.Directory]::CreateDirectory($package) | Out-Null
            $retained = Join-Path $package $name
            [IO.File]::Copy($binary, $retained)
            $manifest = [ordered]@{
                schema = 1
                job = $Job
                commit = $Commit
                version = $version
                target = $Target
                binary = $name
                sha256 = (Get-FileHash -LiteralPath $retained -Algorithm SHA256).Hash.ToLowerInvariant()
            }
            $temporaryManifest = Join-Path $package 'manifest.tmp'
            [IO.File]::WriteAllText($temporaryManifest, ($manifest | ConvertTo-Json))
            [IO.File]::Move($temporaryManifest, (Join-Path $package 'manifest.json'))
            Write-Host "Saved $name in $package."
        } finally {
            Pop-Location
        }
    } finally {
        $env:CARGO_TARGET_DIR = $previousTarget
        try {
            $resolvedWork = (Resolve-Path -LiteralPath $work).ProviderPath
            if ($resolvedWork -ne $work -or (Split-Path -LiteralPath $resolvedWork) -ne $JobDirectory -or
                ((Get-Item -LiteralPath $resolvedWork).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
                throw 'The directory did not match the build directory.'
            }
            Remove-Item -LiteralPath $resolvedWork -Recurse -Force
        } catch {
            Write-Warning "Cannot remove the build directory at $work. $_"
        }
    }
}

function Install-Package($Package, $Job, $InstallDir) {
    $manifestPath = Join-Path $Package 'manifest.json'
    if (!(Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
        throw "The checked package for $Job is unavailable."
    }
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    $version = $manifest.version
    $name = "action-$version-$Target.exe"
    if ($manifest.schema -ne 1 -or $manifest.job -cne $Job -or $manifest.target -cne $Target -or
        $version -notmatch "^$VersionPattern`$" -or $manifest.binary -cne $name -or
        $manifest.commit -notmatch '^(?:[0-9a-f]{40}|[0-9a-f]{64})$' -or
        $manifest.sha256 -notmatch '^[0-9a-f]{64}$') {
        throw "Invalid package manifest for $Job."
    }

    Invoke-Native ffprobe.exe -version | Out-Null
    $encoders = Invoke-Native ffmpeg.exe -hide_banner -encoders
    if (!($encoders -match '^\s*\S{6}\s+libx264\s') -or !($encoders -match '^\s*\S{6}\s+aac\s')) {
        throw 'Use an FFmpeg build with the libx264 and AAC encoders.'
    }

    [IO.Directory]::CreateDirectory($InstallDir) | Out-Null
    $destination = Join-Path $InstallDir 'action.exe'
    $temporary = Join-Path $InstallDir ('.action-' + [Guid]::NewGuid().ToString('N') + '.exe')
    try {
        [IO.File]::Copy((Join-Path $Package $name), $temporary)
        $digest = (Get-FileHash -LiteralPath $temporary -Algorithm SHA256).Hash
        if ($digest -ne $manifest.sha256) {
            throw "The binary checksum must match the manifest for $Job."
        }
        if ((Invoke-Native $temporary --version) -cne "action $version") {
            throw "The binary must report version $version."
        }
        if ([IO.File]::Exists($destination)) {
            [IO.File]::Replace($temporary, $destination, [NullString]::Value)
        } else {
            [IO.File]::Move($temporary, $destination)
        }
        Write-Host "Installed Action $version from job $Job at $destination."
    } finally {
        [IO.File]::Delete($temporary)
    }
}

if ($env:OS -ne 'Windows_NT') {
    throw 'Run this script on Windows.'
}
if (($Mode -eq 'deploy' -and $From -cnotmatch '^[0-9a-f]{32}$') -or ($Mode -ne 'deploy' -and $From)) {
    throw 'Use ./ci.ps1 deploy -From JOB to install a retained Windows binary.'
}

Push-Location -LiteralPath $PSScriptRoot
try {
    $ciRoot = Get-ExternalPath $env:ACTION_CI_ROOT (Join-Path $env:LOCALAPPDATA 'Action\ci') 'ACTION_CI_ROOT'
    $artifacts = Join-Path $ciRoot "$(Get-RepositoryKey)\artifacts"
    if ($Mode -eq 'deploy') {
        $job = $From
    } else {
        if (Invoke-Native git.exe status --porcelain=v1 --untracked-files=all) {
            throw 'Commit the changes before you submit CI. Use a clean Git worktree.'
        }
        $commit = Invoke-Native git.exe rev-parse HEAD
        $job = [Guid]::NewGuid().ToString('N')
        $jobDirectory = Join-Path $artifacts $job
        Write-Host "Windows CI job $job for commit $commit. Artifacts: $jobDirectory"
        New-Package $jobDirectory $job $commit
    }

    if ($Mode -ne 'check') {
        $installDir = Get-ExternalPath $env:ACTION_INSTALL_DIR (Join-Path $env:LOCALAPPDATA 'Action\bin') `
            'ACTION_INSTALL_DIR'
        Install-Package (Join-Path $artifacts "$job\package") $job $installDir
    }
} finally {
    Pop-Location
}
