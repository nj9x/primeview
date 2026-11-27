#!/usr/bin/env bash
# Local CI: the same checks as .github/workflows/ci.yml, run on this machine.
#
#   scripts/ci.sh            lint, test, then release builds (native, Windows, web)
#   scripts/ci.sh --quick    lint and test only
#   scripts/ci.sh --package  everything, plus installers (packaging/package.sh)
#
# Windows is cross-compiled with MinGW (x86_64-pc-windows-gnu). One-time setup on macOS:
#   brew install mingw-w64 makensis && rustup target add x86_64-pc-windows-gnu
# Web needs: rustup target add wasm32-unknown-unknown && brew install trunk
set -euo pipefail

cd "$(dirname "$0")/.."
export CARGO_TERM_COLOR=always

QUICK=0
PACKAGE=0
for arg in "$@"; do
  case "$arg" in
    --quick) QUICK=1 ;;
    --package) PACKAGE=1 ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

WIN=x86_64-pc-windows-gnu
has_windows() {
  command -v x86_64-w64-mingw32-gcc >/dev/null && rustup target list --installed | grep -qx "$WIN"
}
has_web() {
  command -v trunk >/dev/null && rustup target list --installed | grep -qx wasm32-unknown-unknown
}

results=()
step() {
  local name="$1"; shift
  printf '\n\033[1;34m==> %s\033[0m\n' "$name"
  local start=$SECONDS
  if "$@"; then
    results+=("ok    $name ($((SECONDS - start))s)")
  else
    results+=("FAIL  $name")
    summary
    exit 1
  fi
}
skip() { results+=("skip  $1 ($2)"); }
summary() {
  printf '\n\033[1m%s\033[0m\n' "Summary"
  printf '  %s\n' "${results[@]}"
}

step "Lint (native)" cargo clippy --workspace --all-targets -- -D warnings
if has_windows; then
  step "Lint (Windows)" cargo clippy --workspace --all-targets --target "$WIN" -- -D warnings
else
  skip "Lint (Windows)" "needs mingw-w64 and the $WIN target"
fi
step "Lint (web)" cargo clippy -p primeview --target wasm32-unknown-unknown -- -D warnings
step "Tests" cargo test --workspace

if [[ $QUICK == 0 ]]; then
  step "Release build (native)" cargo build --release -p primeview -p primeview-server
  if has_windows; then
    step "Release build (Windows)" cargo build --release -p primeview -p primeview-server --target "$WIN"
  else
    skip "Release build (Windows)" "needs mingw-w64 and the $WIN target"
  fi
  if has_web; then
    step "Release build (web)" bash -c 'cd app && trunk build --release'
  else
    skip "Release build (web)" "needs trunk and the wasm32 target"
  fi
fi

if [[ $PACKAGE == 1 ]]; then
  step "Installers (this OS)" packaging/package.sh
  if has_windows && command -v makensis >/dev/null; then
    step "Installer (Windows)" packaging/package.sh --target "$WIN"
  else
    skip "Installer (Windows)" "needs mingw-w64, the $WIN target and makensis"
  fi
fi

summary
if [[ $QUICK == 0 ]]; then
  echo
  echo "Outputs:"
  echo "  target/release/primeview, target/release/primeview-server"
  if has_windows; then echo "  target/$WIN/release/primeview.exe, primeview-server.exe"; fi
  if has_web; then echo "  dist/ (web app)"; fi
fi
