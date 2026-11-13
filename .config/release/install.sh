#!/bin/sh
#
# Install `jp` from one of JP's releases, on macOS or Linux, from wherever the
# release is published, such as GitHub:
#
#   curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dcdpr/jp/releases/latest/download/install.sh | sh
#
# Downloads the archive for this machine and `checksums.txt` from the address
# the publisher filled in below, refuses an archive whose SHA-256 doesn't match,
# and installs `jp` into Cargo's bin directory, `$CARGO_HOME/bin` or
# `~/.cargo/bin`, where `cargo install` puts it.
#
# JP_DOWNLOAD names another address to download from, such as `file://` and the
# directory `jp release build` kept a build in, to try the script before
# anything is published:
#
#   JP_DOWNLOAD="file://$(jp path user-workspace)/release/0.3.0" sh install.sh
#
# Windows has install.ps1.

set -eu

# Everything runs from `main`, called on the last line, so a download cut short
# runs nothing.

die() {
  echo "install.sh: $*" >&2
  exit 1
}

# The target triple of JP's build for this machine.
target() {
  case "$(uname -s)" in
    Darwin) os=apple-darwin ;;
    Linux) os=unknown-linux-gnu ;;
    *) die "JP has no build for $(uname -s); on Windows, run install.ps1" ;;
  esac

  case "$(uname -m)" in
    arm64 | aarch64) arch=aarch64 ;;
    x86_64 | amd64) arch=x86_64 ;;
    *) die "JP has no build for $(uname -m)" ;;
  esac

  # A shell running under Rosetta reports x86_64 on an Apple silicon Mac, which
  # runs the arm64 build natively.
  if [ "$os" = apple-darwin ] && [ "$arch" = x86_64 ] &&
    [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
    arch=aarch64
  fi

  # The Linux builds link against glibc, which musl systems such as Alpine
  # don't have.
  if [ "$os" = unknown-linux-gnu ] && ldd --version 2>&1 | grep -qi musl; then
    die "JP's Linux builds need glibc 2.28 or newer, and this system uses musl"
  fi

  echo "$arch-$os"
}

# Download URL to FILE.
fetch() {
  case $1 in
    file://*) cp "${1#file://}" "$2" ;;
    *)
      if command -v curl >/dev/null 2>&1; then
        curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL "$1" -o "$2"
      elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
      else
        die "install curl or wget to download JP"
      fi
      ;;
  esac
}

# The SHA-256 of FILE, in lowercase hex.
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  else
    die "install sha256sum or shasum to check what is downloaded"
  fi
}

main() {
  # Where the release's files are served from, which its publisher writes in.
  published='{download}'
  download=${JP_DOWNLOAD:-$published}
  case $download in
    https://* | file://*) download=${download%/} ;;
    *) die "no address to download JP from: run the install.sh a release publishes, or name one in JP_DOWNLOAD" ;;
  esac

  target=$(target)
  archive=jp-$target.tar.xz

  tmp=$(mktemp -d 2>/dev/null || mktemp -d -t jp-install)
  trap 'rm -rf "$tmp"' EXIT

  fetch "$download/checksums.txt" "$tmp/checksums.txt" ||
    die "couldn't download $download/checksums.txt"
  fetch "$download/$archive" "$tmp/$archive" ||
    die "couldn't download $download/$archive"

  expected=$(awk -v file="$archive" '$2 == file || $2 == "*" file { print $1; exit }' "$tmp/checksums.txt")
  [ -n "$expected" ] || die "checksums.txt lists no $archive"
  [ "$(sha256 "$tmp/$archive")" = "$expected" ] ||
    die "$archive doesn't match its SHA-256 in checksums.txt, so nothing was installed"

  # GNU tar runs `xz` to read the archive; the BSD tar macOS ships reads it
  # itself.
  if tar --version 2>/dev/null | grep -q 'GNU tar' && ! command -v xz >/dev/null 2>&1; then
    die "install xz (xz-utils) to unpack $archive"
  fi
  tar -xJf "$tmp/$archive" -C "$tmp" || die "couldn't unpack $archive"

  bin=${CARGO_HOME:-$HOME/.cargo}/bin
  mkdir -p "$bin"
  # Moved into place rather than written over, so a `jp` that is running keeps
  # the file it started from.
  cp "$tmp/jp-$target/jp" "$bin/.jp.new"
  chmod 755 "$bin/.jp.new"
  mv -f "$bin/.jp.new" "$bin/jp"

  echo "Installed $("$bin/jp" --version 2>/dev/null || echo jp) as $bin/jp."
  case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "$bin isn't on your PATH; add it there to run jp." ;;
  esac
}

main "$@"
