#!/usr/bin/env bash
# Build a release binary with the build machine's paths kept out of it.
#
# Rust bakes source paths into the binary at compile time, through `file!()` and through the `log`
# crate's `log.file` field. Every crash report and every `ruffle.log` a player sends back therefore
# quotes the home directory of whoever built the binary, including their account name. A tester
# reported reading a stranger's Windows username throughout his own logs; that is this, and it is a
# property of the build rather than of anything he did.
#
# The profile-level `trim-paths` that would express this in Cargo.toml is still unstable as of Cargo
# 1.96, so the same remapping is passed to rustc directly.
#
# CARGO_ENCODED_RUSTFLAGS rather than RUSTFLAGS on purpose: RUSTFLAGS splits on spaces, and the
# account name that prompted this fix has spaces in it, so the plain form would silently mangle the
# very path it is meant to remove. The encoded form is separated by \x1f and has no such problem.
#
# Usage: scripts/build-release.sh [extra cargo args...]
#   scripts/build-release.sh --features metrics
set -euo pipefail

# `pwd -W` where it exists: under Git Bash a plain `pwd` yields an MSYS path like /e/CLAUDE/Aether,
# which rustc never compares against, so the remap silently did nothing.
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && { pwd -W 2>/dev/null || pwd; })"
cargo_home="${CARGO_HOME:-${USERPROFILE:-$HOME}/.cargo}"
# The toolchain lives under the user profile too, and with debug info on, rustc rewrites the
# standard library's paths to point at it. That leaked the account name 528 times into a build the
# other two prefixes had already cleaned.
rustup_home="${RUSTUP_HOME:-${USERPROFILE:-$HOME}/.rustup}"

# Forward slashes: this is what rustc compares against, even on Windows.
to_rustc_path() { printf '%s' "${1//\\//}"; }

separator=$'\x1f'
flags=(
    "--remap-path-prefix=$(to_rustc_path "$cargo_home")=/cargo"
    "--remap-path-prefix=$(to_rustc_path "$repo_root")=/aether"
    "--remap-path-prefix=$(to_rustc_path "$rustup_home")=/rust"
    "--remap-path-prefix=${rustup_home}=/rust"
)

encoded=""
for flag in "${flags[@]}"; do
    encoded="${encoded:+${encoded}${separator}}${flag}"
done

CARGO_ENCODED_RUSTFLAGS="$encoded" \
    cargo build --release -p aether_player "$@"

printf '\nBuilt with paths remapped:\n'
printf '  %s -> /cargo\n' "$cargo_home"
printf '  %s -> /aether\n' "$repo_root"
printf '  %s -> /rust\n' "$rustup_home"
