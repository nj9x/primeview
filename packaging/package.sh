#!/usr/bin/env bash
# Build PrimeView installers with cargo-packager from an already-compiled binary.
#
#   cargo build --release -p primeview
#   packaging/package.sh                      # formats for this OS
#   packaging/package.sh -f dmg               # just one format
#   packaging/package.sh --target universal-apple-darwin   # after lipo (see README)
#
# Options
#   -f, --formats LIST     comma-separated cargo-packager formats
#                          (default: app,dmg on macOS; nsis,wix on Windows; deb,appimage on Linux)
#   --target TRIPLE        package target/<TRIPLE>/release/primeview and name artifacts for it
#   --binaries-dir DIR     directory holding the `primeview` binary (overrides --target lookup)
#   -o, --out-dir DIR      where packages go (default: <target dir>/packager)
#
# Environment
#   CARGO_TARGET_DIR                 honoured like cargo does (default: <repo>/target)
#   APPLE_SIGNING_IDENTITY           macOS codesign identity; unset => ad-hoc ("-") signature
#   APPLE_CERTIFICATE(_PASSWORD)     base64 .p12 + password, imported into a temp keychain by cargo-packager
#   APPLE_ID, APPLE_PASSWORD, APPLE_TEAM_ID   notarization (only used with a real identity)
#   WINDOWS_CERTIFICATE_THUMBPRINT   SHA-1 thumbprint of a cert in the Windows cert store => signtool signing
#
# Requires: cargo-packager (`cargo install cargo-packager --locked`).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"

formats=""
target=""
binaries_dir=""
out_dir=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -f|--formats) formats="$2"; shift 2 ;;
    --target) target="$2"; shift 2 ;;
    --binaries-dir) binaries_dir="$2"; shift 2 ;;
    -o|--out-dir) out_dir="$2"; shift 2 ;;
    -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
    *) echo "package.sh: unknown argument: $1" >&2; exit 2 ;;
  esac
done

case "$(uname -s)" in
  Darwin) os=macos; default_formats="app,dmg" ;;
  Linux) os=linux; default_formats="deb,appimage" ;;
  MINGW*|MSYS*|CYGWIN*) os=windows; default_formats="nsis,wix" ;;
  *) echo "package.sh: unsupported OS $(uname -s)" >&2; exit 1 ;;
esac
formats="${formats:-$default_formats}"

# Absolute path, and in the native form cargo-packager (a Windows exe on Windows) understands.
abspath() {
  local p="$1"
  [[ "${2:-}" == create ]] && mkdir -p "$p"
  [[ -d "$p" ]] || { echo "package.sh: directory not found: $p" >&2; exit 1; }
  p="$(cd "$p" && pwd)"
  if command -v cygpath >/dev/null 2>&1; then cygpath -m "$p"; else printf '%s\n' "$p"; fi
}

target_dir="${CARGO_TARGET_DIR:-$repo/target}"
if [[ -z "$binaries_dir" ]]; then
  if [[ -n "$target" ]]; then binaries_dir="$target_dir/$target/release"; else binaries_dir="$target_dir/release"; fi
fi
binaries_dir="$(abspath "$binaries_dir")"
out_dir="$(abspath "${out_dir:-$target_dir/packager}" create)"

exe="primeview"; [[ $os == windows ]] && exe="primeview.exe"
if [[ ! -f "$binaries_dir/$exe" ]]; then
  echo "package.sh: $binaries_dir/$exe not found - run 'cargo build --release -p primeview' first" >&2
  exit 1
fi

# Version straight from Cargo so the installers can't drift from the crate.
pkgid="$(cd "$repo" && cargo pkgid -p primeview)"
version="${pkgid##*[@#]}"

# Generated config lives next to Packager.toml so its relative paths still resolve.
gen="$here/.packager.generated.toml"
trap 'rm -f "$gen"' EXIT

# CI maps missing secrets to empty strings; cargo-packager treats a *set* but empty
# var as configured, so drop empty ones.
for v in APPLE_SIGNING_IDENTITY APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_ID APPLE_PASSWORD \
         APPLE_TEAM_ID APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH APPLE_KEYCHAIN_PROFILE \
         WINDOWS_CERTIFICATE_THUMBPRINT; do
  if [[ -z "${!v:-}" ]]; then unset "$v"; fi
done

mac_identity=""
if [[ $os == macos ]]; then
  mac_identity="${APPLE_SIGNING_IDENTITY:-}"
  if [[ -z "$mac_identity" ]]; then
    # Ad-hoc signature: keeps the bundle's seal valid on Apple Silicon (otherwise
    # Gatekeeper reports the downloaded app as "damaged"). Never notarize ad-hoc builds.
    mac_identity="-"
    unset APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID \
      APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH APPLE_KEYCHAIN_PROFILE || true
  fi
fi
win_thumbprint="${WINDOWS_CERTIFICATE_THUMBPRINT:-}"

# TOML literal strings ('...') need no escaping; reject the one char they can't hold.
for v in "$binaries_dir" "$mac_identity" "$win_thumbprint"; do
  [[ "$v" == *"'"* ]] && { echo "package.sh: value contains a single quote: $v" >&2; exit 1; }
done

awk -v version="$version" -v bindir="$binaries_dir" -v os="$os" \
    -v ident="$mac_identity" -v thumb="$win_thumbprint" '
  /^version = /       { print "version = \"" version "\""; next }
  /^binaries-dir = /  { print "binaries-dir = '\''" bindir "'\''"; next }
  /^license-file = /  { if (os == "macos") next }
  { print }
  /^\[macos\]$/   && ident != "" { print "signing-identity = '\''" ident "'\''" }
  /^\[windows\]$/ && thumb != "" { print "certificate-thumbprint = '\''" thumb "'\''" }
' "$here/Packager.toml" > "$gen"

gen_native="$gen"
command -v cygpath >/dev/null 2>&1 && gen_native="$(cygpath -m "$gen")"
args=(packager -c "$gen_native" -f "$formats" -o "$out_dir")
[[ -n "$target" ]] && args+=(--target "$target")

echo "package.sh: PrimeView $version | formats: $formats | binaries: $binaries_dir | out: $out_dir"
cargo "${args[@]}"
