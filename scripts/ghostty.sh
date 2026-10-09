#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (C) 2026 Ronan Lamour
# Builds libghostty-vt, the terminal engine of recruit's multiplexer, and points Cargo at it. Zig, Ghostty's sources
# and Ghostty's Zig packages are fetched once, checked, and kept in .ghostty/ (not in git); the library is built for
# each Rust target asked; .cargo/config.toml (not in git either) then tells Cargo to link it instead of running the
# build script of libghostty-vt-sys (`links = "ghostty-vt"`), so that `cargo build` needs neither Zig nor the network.
#
#   scripts/ghostty.sh                                   the host's target
#   scripts/ghostty.sh x86_64-unknown-linux-musl …       these targets (release.sh: the four it publishes)
#
# Needs curl, tar with xz, and shasum or sha256sum; the network on its first run only. Without it, Cargo falls back on
# the crate's own build, which wants Zig 0.16 first on the PATH and clones Ghostty.
set -euo pipefail

ZIG_VERSION=0.16.0
# The Ghostty commit of the libghostty-rs revision in Cargo.toml (GHOSTTY_COMMIT in its libghostty-vt-sys/build.rs):
# they move together, with the Zig version that commit asks for (minimum_zig_version in its build.zig.zon).
GHOSTTY_COMMIT=22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018
# GitHub's archive of that commit. GitHub does not promise its archives byte for byte: a new sum fails below.
GHOSTTY_SHA256=5fdb21d3744ce76dfaefb0e25fd113540906939752627a4193b88520e126c389

root=$(cd "$(dirname "$0")/.." && pwd)
dir=$root/.ghostty
downloads=$dir/downloads

die() {
  echo "ghostty: $*" >&2
  exit 1
}

sha256() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# Downloads $1 into $2 unless $2 is already there with the sum $3; a different sum is an error.
fetch() {
  local url=$1 file=$2 sum=$3
  if [ -f "$file" ] && [ "$(sha256 "$file")" = "$sum" ]; then
    return
  fi
  mkdir -p "$(dirname "$file")"
  curl -fsSL --retry 3 -o "$file.part" "$url" || die "cannot download $url"
  local got
  got=$(sha256 "$file.part")
  if [ "$got" != "$sum" ]; then
    rm -f "$file.part"
    die "$url: sha256 $got, expected $sum.
  If it is GitHub's archive of Ghostty that changed (GitHub may recompress them), check the new one against the
  commit, then update GHOSTTY_SHA256 in scripts/ghostty.sh; anything else, do not go on."
  fi
  mv "$file.part" "$file"
}

# Extracts the archive $1 into the directory $2, without its top directory.
extract() {
  rm -rf "$2.part"
  mkdir -p "$2.part"
  tar -xf "$1" -C "$2.part" --strip-components 1
  rm -rf "$2"
  mv "$2.part" "$2"
}

# Zig for this machine, from ziglang.org (sums of https://ziglang.org/download/index.json).
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) zig_host=aarch64-macos zig_sum=b23d70deaa879b5c2d486ed3316f7eaa53e84acf6fc9cc747de152450d401489 ;;
  Darwin-x86_64) zig_host=x86_64-macos zig_sum=0387557ed1877bc6a2e1802c8391953baddba76081876301c522f52977b52ba7 ;;
  Linux-x86_64) zig_host=x86_64-linux zig_sum=70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00 ;;
  Linux-aarch64) zig_host=aarch64-linux zig_sum=ea4b09bfb22ec6f6c6ceac57ab63efb6b46e17ab08d21f69f3a48b38e1534f17 ;;
  *) die "no Zig $ZIG_VERSION here ($(uname -sm))" ;;
esac
zig_dir=$dir/zig-$ZIG_VERSION
if [ ! -x "$zig_dir/zig" ]; then
  archive=$downloads/zig-$zig_host-$ZIG_VERSION.tar.xz
  fetch "https://ziglang.org/download/$ZIG_VERSION/zig-$zig_host-$ZIG_VERSION.tar.xz" "$archive" "$zig_sum"
  extract "$archive" "$zig_dir"
  rm -f "$archive"
fi
zig=$zig_dir/zig
[ "$("$zig" version)" = "$ZIG_VERSION" ] || die "$zig is not Zig $ZIG_VERSION"
# The same Zig links the Linux binaries (cargo-zigbuild, CARGO_ZIGBUILD_ZIG_PATH): at a path that does not move with
# its version.
mkdir -p "$dir/bin"
ln -sfn "$zig" "$dir/bin/zig"

# Ghostty's sources.
src=$dir/src-$GHOSTTY_COMMIT
if [ ! -f "$src/build.zig" ]; then
  archive=$downloads/ghostty-$GHOSTTY_COMMIT.tar.gz
  fetch "https://github.com/ghostty-org/ghostty/archive/$GHOSTTY_COMMIT.tar.gz" "$archive" "$GHOSTTY_SHA256"
  extract "$archive" "$src"
  rm -f "$archive"
fi

# What libghostty-vt asks of Zig's build, here and below.
options=(-Demit-lib-vt=true -Dapp-runtime=none -Demit-xcframework=false)

# The Zig packages libghostty-vt needs, fetched by Zig itself, which checks each against its hash in build.zig.zon
# (part of the sources checked above), into $src/zig-pkg. The builds below do not go out.
export ZIG_GLOBAL_CACHE_DIR=$dir/zig-cache
(cd "$src" && "$zig" build --fetch "${options[@]}") >/dev/null || die "cannot fetch Ghostty's Zig packages"

# recruit moves an engine from thread to thread behind a mutex (`unsafe impl Send` in src/mux/engine/ghostty.rs),
# which holds as long as libghostty-vt keeps no terminal state per thread. Checked twice: in the sources here, then in
# each library built (below). Source files known to declare some, none of it in the library built: Ghostty's crash
# reporter, OpenGL loader and fuzzing harness, stb_image, and highway's profiler, thread pool and vqsort.
known_sources='/src/crash/sentry.zig$|/pkg/opengl/glad.zig$|/pkg/afl\+\+/afl.c$|/src/stb/stb_image.h$|/hwy/profiler.h$|/hwy/contrib/thread_pool/futex.h$|/hwy/contrib/sort/vqsort-inl.h$'
found=$(grep -rlE --include='*.zig' --include='*.c' --include='*.h' --include='*.cc' --include='*.cpp' \
  '\b(threadlocal|thread_local|_Thread_local|__thread)\b' "$src/src" "$src/pkg" "$src/zig-pkg" | grep -vE "$known_sources" || true)
[ -z "$found" ] || die "thread-local state in Ghostty $GHOSTTY_COMMIT, see src/mux/engine/ghostty.rs before going on:
$found"

# The thread-local variables a library built may hold: Zig's standard library's, none of them a terminal's state (see
# src/mux/engine/ghostty.rs, symbol by symbol). Any other stops here.
known_tls=' debug.panic_stage Io.Threaded.Thread.current Thread.maybeAttachSignalStack.global.signal_stack Thread.LinuxThreadImpl.tls_thread_id.0 Thread.LinuxThreadImpl.tls_thread_id.1 '
thread_locals() {
  case "$2" in
    *-apple-darwin) nm -m "$1" | awk '/\(__DATA,__thread_vars\)/ { print $NF }' | sed 's/^_//' ;;
    *) objdump -t "$1" | awk '/[ \t]\.(tbss|tdata)[ \t]/ { print $NF }' ;;
  esac | grep -vE '^(ltmp[0-9]+|\$[a-z]+)$' | sort -u
}
check_thread_locals() {
  local symbol
  for symbol in $(thread_locals "$1" "$2"); do
    case "$known_tls" in
      *" $symbol "*) ;;
      *) die "$2: thread-local $symbol in libghostty-vt, see src/mux/engine/ghostty.rs before going on" ;;
    esac
  done
}

# macOS: the oldest version Rust builds for (`rustc --print deployment-target`), not Ghostty's 13.0, so that
# recruit runs where it ran before.
zig_target() {
  case "$1" in
    aarch64-apple-darwin) echo aarch64-macos.11.0 ;;
    x86_64-apple-darwin) echo x86_64-macos.10.12 ;;
    aarch64-unknown-linux-musl) echo aarch64-linux-musl ;;
    x86_64-unknown-linux-musl) echo x86_64-linux-musl ;;
    aarch64-unknown-linux-gnu) echo aarch64-linux-gnu ;;
    x86_64-unknown-linux-gnu) echo x86_64-linux-gnu ;;
    *) die "no Zig target known for $1" ;;
  esac
}

host=$(rustc -vV | sed -n 's/^host: //p')
[ $# -gt 0 ] || set -- "$host"
for triple in "$@"; do
  out=$dir/$GHOSTTY_COMMIT/$triple
  [ -f "$out/lib/libghostty-vt.a" ] && continue
  echo "ghostty: building libghostty-vt for $triple" >&2
  # As libghostty-vt-sys builds it: ReleaseFast, the CPU every machine of that target has, no app.
  (cd "$src" && "$zig" build "${options[@]}" -Doptimize=ReleaseFast -Dcpu=baseline -Dtarget="$(zig_target "$triple")" \
    --prefix "$out.part" --cache-dir "$dir/zig-local") || die "cannot build libghostty-vt for $triple"
  [ -f "$out.part/lib/libghostty-vt.a" ] || die "no libghostty-vt.a for $triple"
  check_thread_locals "$out.part/lib/libghostty-vt.a" "$triple"
  rm -rf "$out"
  mv "$out.part" "$out"
done
# The intermediate files of the builds, several hundred megabytes, are of no more use.
rm -rf "$dir/zig-local"

# Cargo links what was built, for every target built so far, instead of running libghostty-vt-sys's build script.
mkdir -p "$root/.cargo"
{
  echo "# Written by scripts/ghostty.sh (libghostty-vt of Ghostty $GHOSTTY_COMMIT, Zig $ZIG_VERSION): not in git."
  for lib in "$dir/$GHOSTTY_COMMIT"/*/lib/libghostty-vt.a; do
    [ -f "$lib" ] || continue
    triple=$(basename "$(dirname "$(dirname "$lib")")")
    printf '\n[target.%s.ghostty-vt]\nrustc-link-lib = ["static=ghostty-vt"]\nrustc-link-search = ["native=%s"]\n' \
      "$triple" "$(dirname "$lib")"
  done
} >"$root/.cargo/config.toml.part"
mv "$root/.cargo/config.toml.part" "$root/.cargo/config.toml"
echo "ghostty: libghostty-vt ready for: $(ls "$dir/$GHOSTTY_COMMIT" | tr '\n' ' ')" >&2
