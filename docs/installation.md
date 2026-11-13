# Installation

JP runs on macOS (Apple silicon and Intel), Linux (x86_64 and ARM64), and
Windows (x86_64).

## Install Script

On macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dcdpr/jp/releases/latest/download/install.sh | sh
```

On Windows, in PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/dcdpr/jp/releases/latest/download/install.ps1 | iex"
```

The installer downloads the latest release archive for your platform, verifies
its SHA-256 checksum against `checksums.txt`, and installs `jp` into
`~/.cargo/bin`.
If `~/.cargo/bin` is not in your `PATH`, the installer prints instructions to
add it.

## Manual Download

Each [release][releases] provides prebuilt archives and a `checksums.txt` file
containing their SHA-256 digests:

| Platform               | Archive                               |
| ---------------------- | ------------------------------------- |
| macOS on Apple silicon | `jp-aarch64-apple-darwin.tar.xz`      |
| macOS on Intel         | `jp-x86_64-apple-darwin.tar.xz`       |
| Linux on ARM64         | `jp-aarch64-unknown-linux-gnu.tar.xz` |
| Linux on x86_64        | `jp-x86_64-unknown-linux-gnu.tar.xz`  |
| Windows on x86_64      | `jp-x86_64-pc-windows-msvc.zip`       |

Download your platform's archive and `checksums.txt` into the same directory and
verify the checksum.
On Linux:

```sh
sha256sum -c --ignore-missing checksums.txt
```

On macOS:

```sh
shasum -a 256 -c --ignore-missing checksums.txt
```

On Windows, compare the output of `Get-FileHash jp-x86_64-pc-windows-msvc.zip`
against the hash in `checksums.txt`.

Extract the archive and place `jp` (or `jp.exe` on Windows) in a directory on
your `PATH`.

Linux binaries require glibc 2.28 or newer (Debian 10+, Ubuntu 20.04+, RHEL 8+).
Systems using musl (such as Alpine Linux) must [install from
source](#install-from-source).

### Downloads From a Browser

Release binaries are not code-signed or notarized.
When you download an archive in a web browser, macOS attaches a quarantine
attribute and blocks execution (`curl` and the install script do not set this
attribute).
Clear the quarantine attribute with `xattr`:

```sh
xattr -d com.apple.quarantine jp
```

Windows SmartScreen may show a similar warning for `jp.exe` when downloaded via
a browser.

## Install From Source

With [Cargo] and a recent stable Rust toolchain:

```sh
cargo install --locked --git https://github.com/dcdpr/jp.git --tag v0.3.0 jp_cli
```

Omit `--tag v0.3.0` to build the latest commit on `main`.

[Cargo]: https://doc.rust-lang.org/cargo/
[releases]: https://github.com/dcdpr/jp/releases
