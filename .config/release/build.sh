#!/usr/bin/env bash
#
# Build JP's release binaries: `jp` for every target JP ships, from one
# release's code, on this Mac. `jp release build` runs it from the workspace
# root, as `plugins.command.release.options.build` in `.jp/config.toml` says:
#
#   bash .config/release/build.sh VERSION SOURCE OUT PROFILE
#
# VERSION is the release's version, SOURCE a checkout of its code, OUT the empty
# directory the build goes in, and PROFILE `release`, for the binaries a release
# ships, or `quick`, Cargo's default release profile, to test a build in less
# time.
#
# OUT gets an archive per target, jp-<target>.tar.xz, or jp-<target>.zip for
# Windows, holding a directory of the same name with `jp` and the README.md and
# LICENSE that SOURCE has; checksums.txt, a SHA-256 per archive in sha256sum's
# format; and manifest.json, which names the version, commit, profile, and each
# archive's target. `jp release build` checks all three before keeping them.
#
# The targets are macOS and Linux on arm64 and x86_64, and Windows on x86_64
# from #428 on, which added Windows to CI: no Windows build of an older commit
# was ever run. macOS builds with Apple's toolchain, Linux with cargo-zigbuild
# against glibc 2.28, and Windows with cargo-xwin, with the C runtime linked in.
#
# The toolchain is SOURCE's `[workspace.metadata.release] toolchain` in
# Cargo.toml, an exact stable version, or else the nightly its
# rust-toolchain.toml pins, which its CI tested. The profile is set through
# CARGO_PROFILE_RELEASE_* and the rustflags through the environment, so neither
# SOURCE's Cargo.toml nor a Cargo configuration file changes what is built, and
# this machine's paths are remapped out of the binaries. SOURCE_DATE_EPOCH is the
# commit's time, which `jp --version` shows and the archived files carry.
#
# JP_RELEASE_JOBS     How many targets build at once; 2 by default. A release
#                     build's link-time optimization takes several GB of memory
#                     per target.
# JP_RELEASE_TARGETS  The targets a quick build builds, such as `host` for this
#                     Mac's; every target by default.
# JP_RELEASE_CACHE    Where each target's builds are kept between runs, with the
#                     logs of the last; ~/Library/Caches/jp-release by default.
#                     Delete it to reclaim the space.
#
# What each kind of target needs installed:
#
#   macOS    Xcode or the Command Line Tools, with an ld as new as their SDK
#   Linux    brew install zig && cargo install --locked cargo-zigbuild
#   Windows  brew install llvm && cargo install --locked cargo-xwin
#            (cargo-xwin downloads Microsoft's CRT and Windows SDK, under
#            Microsoft's license)
#
# Runs on the bash macOS ships, 3.2.

set -euo pipefail

die() {
  echo "build.sh: $*" >&2
  exit 1
}

# Nothing in the caller's environment shapes the binaries: `build` sets the
# rustflags for each target, and the profile is set below in full.
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS
for variable in $(env | sed -n 's/^\(CARGO_PROFILE_RELEASE_[A-Z0-9_]*\)=.*/\1/p'); do
  unset "$variable"
done

[ $# -eq 4 ] || die "usage: build.sh VERSION SOURCE OUT PROFILE"
version=$1
src=$2
out=$3
profile=$4

# Every target JP ships, in the order the manifest lists them.
ALL="aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu x86_64-pc-windows-msvc"

# The commit of #428, which added Windows to CI.
WINDOWS_SINCE=3669250c0bfb4428a726ac0ef428f1a6cf9b2c39

# The oldest glibc the Linux binaries run on: Debian 10, RHEL 8, and newer.
GLIBC=2.28

# Today's commits and the oldest ones both name the package `jp_cli`.
CARGO_ARGS=(--release --locked -p jp_cli --bin jp)

[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "\`$version\` isn't a release's version, such as 0.3.0"
case $profile in
  release | quick) ;;
  *) die "the profile is \`release\` or \`quick\`, not \`$profile\`" ;;
esac
[ -f "$src/Cargo.toml" ] || die "$src holds no Cargo workspace"
{ [ -d "$out" ] && [ -z "$(ls -A "$out")" ]; } || die "$out isn't an empty directory"

case "$(uname -s) $(uname -m)" in
  "Darwin arm64") host=aarch64-apple-darwin ;;
  "Darwin x86_64") host=x86_64-apple-darwin ;;
  *) die "releases are built on macOS, since its targets need Apple's SDK" ;;
esac

src=$(cd "$src" && pwd -P)
out=$(cd "$out" && pwd -P)
commit=$(git -C "$src" rev-parse HEAD) || die "$src isn't a git checkout"
short=${commit:0:7}
SOURCE_DATE_EPOCH=$(git -C "$src" log -1 --format=%ct HEAD)
export SOURCE_DATE_EPOCH

targets=""
if [ -n "${JP_RELEASE_TARGETS:-}" ]; then
  [ "$profile" = quick ] || die "JP_RELEASE_TARGETS is for quick builds: a release ships every target"
  for target in $JP_RELEASE_TARGETS; do
    if [ "$target" = host ]; then target=$host; fi
    case " $ALL " in
      *" $target "*) ;;
      *) die "unknown target $target; the targets are: $ALL" ;;
    esac
    case " $targets " in
      *" $target "*) ;;
      *) targets="$targets $target" ;;
    esac
  done
  [ -n "$targets" ] || die "JP_RELEASE_TARGETS names no target"
else
  found=0
  git -C "$src" merge-base --is-ancestor "$WINDOWS_SINCE" HEAD 2>/dev/null || found=$?
  case $found in
    0) targets=$ALL ;;
    1)
      targets=${ALL% x86_64-pc-windows-msvc}
      echo "No Windows build: $short is older than #428, which added Windows to CI."
      ;;
    *) die "can't tell whether $short includes #428 ($WINDOWS_SINCE), which added Windows to CI" ;;
  esac
fi
targets=${targets# }

# Homebrew's LLVM is keg-only, so it isn't on PATH; cargo-xwin looks for it
# there itself.
llvm=0
for dir in /opt/homebrew/opt/llvm/bin /usr/local/opt/llvm/bin; do
  if [ -x "$dir/clang" ]; then llvm=1; fi
done
if command -v clang-cl >/dev/null 2>&1; then llvm=1; fi

missing=""
need() { # COMMAND INSTALL: notes INSTALL when COMMAND isn't on PATH
  command -v "$1" >/dev/null 2>&1 || missing="$missing"$'\n'"  $2"
}
need rustup "see https://rustup.rs"
case " $targets " in
  *-linux-gnu*)
    need zig "brew install zig"
    need cargo-zigbuild "cargo install --locked cargo-zigbuild"
    ;;
esac
case " $targets " in
  *-windows-msvc*)
    need cargo-xwin "cargo install --locked cargo-xwin"
    if [ "$llvm" = 0 ]; then missing="$missing"$'\n'"  brew install llvm"; fi
    ;;
esac
[ -z "$missing" ] || die "install what these targets need first:$missing"

scratch=$(mktemp -d "${TMPDIR:-/tmp}/jp-release-build.XXXXXX")
# Each running build, as `PID:TARGET:STARTED`, separated by spaces.
running=""

kill_tree() { # PID: sends SIGTERM to PID's descendants, deepest first, then PID
  local child
  for child in $(pgrep -P "$1" 2>/dev/null || true); do
    kill_tree "$child"
  done
  kill "$1" 2>/dev/null || true
}

# A script's background jobs ignore SIGINT, so on Ctrl-C the builds would go
# on after the script ends: an interruption ends the script, which stops them.
cleanup() {
  local entry
  for entry in $running; do
    kill_tree "${entry%%:*}"
  done
  rm -rf "$scratch"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# An ld older than its SDK can't read it: macOS 27's SDK lists an architecture
# only Xcode 27's ld knows (rust-lang/rust#162928). So every Apple link, build
# scripts included, uses one developer directory whose clang links a test
# program against its own SDK.
apple_dir=""
tried=" "
for dir in ${DEVELOPER_DIR:-} $(xcode-select -p 2>/dev/null || true) \
  /Applications/Xcode.app/Contents/Developer /Library/Developer/CommandLineTools; do
  case $tried in *" $dir "*) continue ;; esac
  tried="$tried$dir "
  [ -d "$dir" ] || continue
  clang=$(DEVELOPER_DIR=$dir xcrun --find clang 2>/dev/null) || continue
  sdk=$(DEVELOPER_DIR=$dir xcrun --sdk macosx --show-sdk-path 2>/dev/null) || continue
  if printf 'int main(void) { return 0; }\n' |
    DEVELOPER_DIR=$dir SDKROOT=$sdk "$clang" -x c - -o "$scratch/probe" >"$scratch/probe.log" 2>&1; then
    apple_dir=$dir
    break
  fi
  echo "Not using $dir: its clang can't link against $sdk:"
  grep -m 2 -E 'error|ld:' "$scratch/probe.log" | sed 's/^/  /' || true
done
[ -n "$apple_dir" ] ||
  die "no Apple developer directory here links a program against its own SDK; update Xcode or the Command Line Tools"

export DEVELOPER_DIR=$apple_dir SDKROOT=$sdk
export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=$clang CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER=$clang
export CC_aarch64_apple_darwin=$clang CC_x86_64_apple_darwin=$clang
apple="$apple_dir (SDK $(xcrun --sdk "$SDKROOT" --show-sdk-version 2>/dev/null || echo '?'), $(xcrun ld -v 2>&1 | sed -n 's/.*PROJECT:\([^ ]*\).*/\1/p' || true))"

toml_string() { # FILE TABLE KEY: the string KEY is set to in [TABLE] of FILE
  [ -f "$1" ] || return 0
  awk -v table="[$2]" -v key="$3" '
    /^[ \t]*\[/ {
      header = $0
      sub(/#.*/, "", header)
      gsub(/[ \t]/, "", header)
      in_table = (header == table)
      next
    }
    in_table {
      line = $0
      sub(/^[ \t]+/, "", line)
      if (substr(line, 1, length(key)) != key) next
      rest = substr(line, length(key) + 1)
      if (rest !~ /^[ \t]*=[ \t]*"/) next
      sub(/^[ \t]*=[ \t]*"/, "", rest)
      sub(/".*/, "", rest)
      print rest
      exit
    }
  ' "$1"
}

toolchain=$(toml_string "$src/Cargo.toml" workspace.metadata.release toolchain)
if [ -z "$toolchain" ]; then
  toolchain=$(toml_string "$src/rust-toolchain.toml" toolchain channel)
fi
[ -n "$toolchain" ] ||
  die "$short names no toolchain: neither \`[workspace.metadata.release] toolchain\` in Cargo.toml nor a channel in rust-toolchain.toml"
if ! [[ $toolchain =~ ^[0-9]+\.[0-9]+\.[0-9]+$ || $toolchain =~ ^(stable|beta|nightly)-[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
  die "$short names the toolchain \`$toolchain\`, which is neither an exact version, such as 1.99.1, nor a dated channel, such as nightly-2025-04-30"
fi

rustup_log=$scratch/rustup.log
if ! rustup run "$toolchain" rustc --version >/dev/null 2>&1; then
  echo "Installing the $toolchain toolchain."
  rustup toolchain install "$toolchain" --profile minimal --no-self-update >"$rustup_log" 2>&1 ||
    { cat "$rustup_log" >&2; die "rustup couldn't install $toolchain"; }
fi
# shellcheck disable=SC2086 # a list of target triples
rustup target add --toolchain "$toolchain" $targets >"$rustup_log" 2>&1 ||
  { cat "$rustup_log" >&2; die "rustup couldn't add the targets to $toolchain"; }

# Every cargo, rustc, and rustup below uses this toolchain, whatever SOURCE's
# rust-toolchain.toml says.
export RUSTUP_TOOLCHAIN=$toolchain

# The whole profile, so SOURCE's `[profile.release]` changes nothing. Unwinding
# stays on: a panic in a spawned task, such as a tool call's, reaches the turn as
# an error rather than ending `jp`.
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_RELEASE_DEBUG=0
export CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=false
export CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=false
export CARGO_PROFILE_RELEASE_PANIC=unwind
export CARGO_PROFILE_RELEASE_INCREMENTAL=false
if [ "$profile" = release ]; then
  export CARGO_PROFILE_RELEASE_OPT_LEVEL=s CARGO_PROFILE_RELEASE_LTO=fat
  export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_STRIP=symbols
else
  export CARGO_PROFILE_RELEASE_OPT_LEVEL=3 CARGO_PROFILE_RELEASE_LTO=false
  export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_STRIP=debuginfo
fi

# Compilers before rust-lang/rust#140872 (Rust 1.89, and nightlies before
# 2025-06-06) don't mark `#[used]` statics as retained on ELF. rustc then asks
# the linker for `-z nostart-stop-gc`, which Zig doesn't support, so
# cargo-zigbuild drops it, and the LLD inside Zig garbage-collects the sections
# `linkme` registers into. Linux builds with those compilers skip linker GC.
rustc_release=$(rustc -vV | sed -n 's/^release: //p')
rustc_date=$(rustc -vV | sed -n 's/^commit-date: //p')
minor=$(printf '%s\n' "$rustc_release" | cut -d. -f2)
linux_flags=""
if [ "$minor" -lt 89 ] || { [ "$minor" -eq 89 ] && [[ $rustc_release == *-nightly && $rustc_date < 2025-06-06 ]]; }; then
  linux_flags="-Clink-dead-code -Clink-arg=-Wl,--no-gc-sections"
fi

cache=${JP_RELEASE_CACHE:-$HOME/Library/Caches/jp-release}
mkdir -p "$cache"
cache=$(cd "$cache" && pwd -P)
cargo_home=${CARGO_HOME:-$HOME/.cargo}
if [ -d "$cargo_home" ]; then cargo_home=$(cd "$cargo_home" && pwd -P); fi
for path in "$cache" "$cargo_home"; do
  case $path in *[[:space:]]*) die "$path has a space in it, which a rustflag can't carry" ;; esac
done

# Panic messages name the files of crates, which would otherwise show where
# this machine keeps Cargo's registry and the builds. SOURCE's own files are
# named relative to it already, and remapping its path, which changes with
# every build, would change the rustflags, and with them every crate's
# fingerprint, so nothing would be reused.
remaps="--remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$cache=/build"

flags_for() { # TARGET: the rustflags TARGET builds with, separated by spaces
  case $1 in
    *-linux-gnu) echo "$remaps${linux_flags:+ $linux_flags}" ;;
    # A static C runtime spares Windows users installing Visual C++'s.
    *-windows-msvc) echo "$remaps -Ctarget-feature=+crt-static" ;;
    *) echo "$remaps" ;;
  esac
}

build() ( # TARGET: builds `jp` for TARGET, in a target directory of its own
  cd "$src" || exit 1
  export CARGO_TARGET_DIR=$cache/$1
  flags=$(flags_for "$1")
  # Rustflags from the environment replace any in Cargo's configuration files.
  # cargo-xwin adds its own to RUSTFLAGS; the others take Cargo's encoding.
  case $1 in
    *-apple-darwin)
      CARGO_ENCODED_RUSTFLAGS=$(printf '%s' "$flags" | tr ' ' '\037') \
        cargo build "${CARGO_ARGS[@]}" --target "$1"
      ;;
    *-linux-gnu)
      CARGO_ENCODED_RUSTFLAGS=$(printf '%s' "$flags" | tr ' ' '\037') \
        cargo zigbuild "${CARGO_ARGS[@]}" --target "$1.$GLIBC"
      ;;
    *-windows-msvc) RUSTFLAGS=$flags cargo xwin build "${CARGO_ARGS[@]}" --target "$1" ;;
  esac
)

binary() { # TARGET: where the build of TARGET leaves `jp`
  case $1 in
    *-windows-msvc) echo "$cache/$1/$1/release/jp.exe" ;;
    *) echo "$cache/$1/$1/release/jp" ;;
  esac
}

first_errors() { # LOG: the log from its first error on, or its end if it has none
  if grep -q '^error' "$1"; then
    awk '/^error/ { found = 1 } found && shown < 40 { print; shown++ }' "$1"
  else
    tail -n 20 "$1"
  fi
}

elapsed() { # SECONDS: such as `8m 42s`
  if [ "$1" -ge 60 ]; then
    echo "$(($1 / 60))m $(($1 % 60))s"
  else
    echo "${1}s"
  fi
}

count() { # WORDS...: how many there are
  echo $#
}

jobs_max=${JP_RELEASE_JOBS:-2}
[[ $jobs_max =~ ^[1-9][0-9]*$ ]] ||
  die "JP_RELEASE_JOBS is how many targets build at once, not \`$jobs_max\`"

logs=$cache/logs/$version
rm -rf "$logs"
mkdir -p "$logs"

echo "Building jp $version at $short with $(rustc --version), $profile profile, $jobs_max at a time."
echo "Apple toolchain: $apple"
echo "Logs: $logs"

# Up to `jobs_max` builds run at once, each writing its exit status to a file
# when it ends, which the loop polls for: the bash macOS ships has no `wait -n`.
pending=$targets
failed=""
while [ -n "$pending" ] || [ -n "$running" ]; do
  # shellcheck disable=SC2086 # counts the entries
  while [ -n "$pending" ] && [ "$(count $running)" -lt "$jobs_max" ]; do
    # shellcheck disable=SC2086 # a list of target triples
    set -- $pending
    target=$1
    shift
    pending=$*
    echo "Building $target."
    (
      status=0
      build "$target" || status=$?
      echo "$status" >"$logs/$target.status.new"
      mv "$logs/$target.status.new" "$logs/$target.status"
    ) >"$logs/$target.log" 2>&1 &
    running="$running $!:$target:$SECONDS"
  done

  sleep 1
  still=""
  for entry in $running; do
    pid=${entry%%:*}
    rest=${entry#*:}
    target=${rest%%:*}
    started=${rest#*:}
    if [ ! -f "$logs/$target.status" ]; then
      still="$still $entry"
      continue
    fi
    wait "$pid" || true
    if [ "$(cat "$logs/$target.status")" = 0 ]; then
      echo "Built $target in $(elapsed $((SECONDS - started)))."
    else
      echo "Failed to build $target after $(elapsed $((SECONDS - started)))."
      failed="$failed $target"
    fi
  done
  running=$still
done

if [ -n "$failed" ]; then
  for target in $failed; do
    echo
    echo "$target's first errors, from $logs/$target.log:"
    first_errors "$logs/$target.log" | sed 's/^/  | /'
  done
  die "failed to build:$failed"
fi

# The binary this Mac runs has to name the version it was built as: a release
# whose `bump` didn't write it would otherwise ship under the wrong one.
case " $targets " in
  *" $host "*)
    said=$("$(binary "$host")" --version 2>&1) || die "\`jp --version\` failed on this Mac: $said"
    echo "jp --version: $said"
    case $said in
      *"v$version-"* | *" $version" | *" $version "*) ;;
      *) die "\`jp --version\` doesn't name $version; did \`bump\` write it?" ;;
    esac
    ;;
esac

# The archives hold the same bytes however often they're made: their files carry
# the commit's time, root's ownership, and no macOS metadata, in the order given.
stage=$scratch/stage
mkdir -p "$stage"
stamp=$(TZ=UTC date -r "$SOURCE_DATE_EPOCH" +%Y%m%d%H%M.%S)
artifacts=""
for target in $ALL; do
  case " $targets " in *" $target "*) ;; *) continue ;; esac

  name=jp-$target
  bin=$(binary "$target")
  mkdir "$stage/$name"
  cp "$bin" "$stage/$name/"
  chmod 755 "$stage/$name" "$stage/$name/${bin##*/}"
  files="$name/${bin##*/}"
  for doc in README.md LICENSE; do
    if [ -f "$src/$doc" ]; then
      cp "$src/$doc" "$stage/$name/$doc"
      chmod 644 "$stage/$name/$doc"
      files="$files $name/$doc"
    fi
  done

  # shellcheck disable=SC2086 # a list of paths without spaces
  (cd "$stage" && TZ=UTC touch -t "$stamp" "$name" $files)
  case $target in
    *-windows-msvc)
      archive=$name.zip
      # shellcheck disable=SC2086 # a list of paths without spaces
      (cd "$stage" && TZ=UTC zip -X -9 -q -D "$out/$archive" $files)
      ;;
    *)
      archive=$name.tar.xz
      # shellcheck disable=SC2086 # a list of paths without spaces
      (cd "$stage" && COPYFILE_DISABLE=1 tar --format ustar --uid 0 --gid 0 --numeric-owner \
        --no-mac-metadata --no-xattrs --no-acls -n -cJ --options xz:compression-level=9 \
        -f "$out/$archive" "$name" $files)
      ;;
  esac
  artifacts="$artifacts $archive:$target"
done

(
  cd "$out"
  for entry in $artifacts; do echo "${entry%%:*}"; done | sort | while read -r file; do
    shasum -a 256 "$file"
  done
) >"$out/checksums.txt"

{
  printf '{\n'
  printf '  "format": 1,\n'
  printf '  "version": "%s",\n' "$version"
  printf '  "commit": "%s",\n' "$commit"
  printf '  "profile": "%s",\n' "$profile"
  printf '  "artifacts": [\n'
  separator=""
  for entry in $artifacts; do
    printf '%s    { "file": "%s", "target": "%s" }' "$separator" "${entry%%:*}" "${entry#*:}"
    separator=$',\n'
  done
  printf '\n  ]\n}\n'
} >"$out/manifest.json"

echo "Packaged jp $version at $short, in $(elapsed "$SECONDS") altogether:"
for entry in $artifacts; do
  file=${entry%%:*}
  bytes=$(wc -c <"$out/$file" | tr -d ' ')
  echo "  $file  $(awk -v bytes="$bytes" 'BEGIN { printf "%.1f MB", bytes / 1000000 }')"
done
