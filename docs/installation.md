# Installation

JP runs on macOS and Linux, on Apple silicon or ARM64 and on x86_64, and on
Windows on x86_64.

## Install Script

On macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dcdpr/jp/releases/latest/download/install.sh | sh
```

On Windows, in PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/dcdpr/jp/releases/latest/download/install.ps1 | iex"
```

The script downloads the latest release for your platform, checks it against the
release's checksums, and installs `jp` into `~/.cargo/bin`, where `cargo
install` puts it.
It tells you when that directory isn't on your `PATH`.

## Manual Download

Each [release][releases] has an archive for each platform, and a `checksums.txt`
with the SHA-256 of each archive:

| Platform               | Archive                               |
| ---------------------- | ------------------------------------- |
| macOS on Apple silicon | `jp-aarch64-apple-darwin.tar.xz`      |
| macOS on Intel         | `jp-x86_64-apple-darwin.tar.xz`       |
| Linux on ARM64         | `jp-aarch64-unknown-linux-gnu.tar.xz` |
| Linux on x86_64        | `jp-x86_64-unknown-linux-gnu.tar.xz`  |
| Windows on x86_64      | `jp-x86_64-pc-windows-msvc.zip`       |

Download your platform's archive and `checksums.txt` into one directory, and
check the archive there.
On Linux:

```sh
sha256sum -c --ignore-missing checksums.txt
```

On macOS:

```sh
shasum -a 256 -c --ignore-missing checksums.txt
```

On Windows, compare the hash `Get-FileHash jp-x86_64-pc-windows-msvc.zip` prints
with the one `checksums.txt` gives the archive.

Each archive holds a directory of the same name, with `jp`, or `jp.exe` on
Windows; put it in a directory on your `PATH`.

The Linux builds need glibc 2.28 or newer, which Debian 10, Ubuntu 20.04, RHEL
8, and their successors have; systems built on musl, such as Alpine, can't run
them.

### Downloads From a Browser

JP's binaries aren't signed.
A browser marks the files it downloads, and macOS refuses to run a marked
program that Apple hasn't notarized, which JP's aren't.
`curl` and the install script leave no such mark, and this removes it from a
`jp` unpacked from an archive a browser downloaded:

```sh
xattr -d com.apple.quarantine jp
```

Windows may warn about a `jp.exe` downloaded through a browser in the same way.

## Install From Source

With [Cargo] and a recent stable Rust:

```sh
cargo install --locked --git https://github.com/dcdpr/jp.git --tag v0.3.0 jp_cli
```

`--tag` names the release to build; without it, Cargo builds the newest code on
`main`.

[Cargo]: https://doc.rust-lang.org/cargo/
[releases]: https://github.com/dcdpr/jp/releases
