# Install `jp` from one of JP's releases, on Windows, from wherever the release
# is published, such as GitHub:
#
#   powershell -ExecutionPolicy Bypass -c "irm https://github.com/dcdpr/jp/releases/latest/download/install.ps1 | iex"
#
# Downloads the Windows archive and `checksums.txt` from the address the
# publisher filled in below, refuses an archive whose SHA-256 doesn't match, and
# installs `jp.exe` into Cargo's bin directory, `$env:CARGO_HOME\bin` or
# `~\.cargo\bin`, where `cargo install` puts it.
#
# JP_DOWNLOAD names another address to download from, such as `file://` and the
# directory `jp release build` kept a build in, to try the script before
# anything is published.
#
# Runs on Windows PowerShell 5.1 and on PowerShell 7. macOS and Linux have
# install.sh.

# `iex` runs a script in the caller's session, so everything is kept inside a
# script block of its own, which leaves the session's settings as they were.
& {
    $ErrorActionPreference = 'Stop'
    # Windows PowerShell 5.1 draws a progress bar that slows downloads down.
    $ProgressPreference = 'SilentlyContinue'

    # Download $Url to $Path.
    function Get-JpFile($Url, $Path) {
        if ($Url -like 'file://*') {
            Copy-Item -LiteralPath ([Uri]$Url).LocalPath -Destination $Path
        } else {
            Invoke-WebRequest -Uri $Url -OutFile $Path -UseBasicParsing
        }
    }

    # The SHA-256 checksums.txt gives $Archive, or $null when it lists none.
    function Get-JpChecksum($Checksums, $Archive) {
        foreach ($line in Get-Content -LiteralPath $Checksums) {
            $fields = $line.Trim() -split '\s+', 2
            if ($fields.Count -eq 2 -and $fields[1].TrimStart('*') -eq $Archive) {
                return $fields[0]
            }
        }

        return $null
    }

    # Where the release's files are served from, which its publisher writes in.
    $published = '{download}'
    $download = $published
    if ($env:JP_DOWNLOAD) {
        $download = $env:JP_DOWNLOAD
    }
    if ($download -notmatch '^(https|file)://') {
        throw 'No address to download JP from: run the install.ps1 a release publishes, or name one in JP_DOWNLOAD.'
    }
    $download = $download.TrimEnd('/')

    # Windows on ARM runs the x86_64 build under emulation.
    $arch = $env:PROCESSOR_ARCHITECTURE
    if ($arch -ne 'AMD64' -and $arch -ne 'ARM64') {
        throw "JP has no build for $arch Windows."
    }
    $target = 'x86_64-pc-windows-msvc'
    $archive = "jp-$target.zip"

    # Windows PowerShell 5.1 may offer only TLS versions older than GitHub accepts.
    $tls = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    [Net.ServicePointManager]::SecurityProtocol = $tls

    $cargo = Join-Path $HOME '.cargo'
    if ($env:CARGO_HOME) {
        $cargo = $env:CARGO_HOME
    }
    $bin = Join-Path $cargo 'bin'
    $jp = Join-Path $bin 'jp.exe'

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ('jp-install-' + [Guid]::NewGuid())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $checksums = Join-Path $tmp 'checksums.txt'
        $zip = Join-Path $tmp $archive
        Get-JpFile "$download/checksums.txt" $checksums
        Get-JpFile "$download/$archive" $zip

        $expected = Get-JpChecksum $checksums $archive
        if (-not $expected) {
            throw "checksums.txt lists no $archive."
        }
        # PowerShell compares strings without regard to case.
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash -ne $expected) {
            throw "$archive doesn't match its SHA-256 in checksums.txt, so nothing was installed."
        }

        Expand-Archive -LiteralPath $zip -DestinationPath $tmp -Force
        New-Item -ItemType Directory -Force -Path $bin | Out-Null
        Copy-Item -LiteralPath (Join-Path $tmp "jp-$target\jp.exe") -Destination $jp -Force
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }

    $version = 'jp'
    try {
        $version = & $jp --version
    } catch {
        # The version is only shown: jp is installed either way.
    }
    Write-Host "Installed $version as $jp."
    if (($env:Path -split ';') -notcontains $bin) {
        Write-Host "$bin isn't on your PATH; add it there to run jp."
    }
}
