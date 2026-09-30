#!/usr/bin/env bash
# Full local verification. Set SKIP_E2E=1 to skip the Playwright step and SKIP_DESKTOP=1 to skip the
# real-webview desktop smoke test (which also needs WebKitWebDriver and xvfb-run).
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }

step "cargo fmt --check"
cargo fmt --all --check

step "cargo clippy (workspace, all targets, -D warnings)"
cargo clippy --workspace --all-targets -- -D warnings

step "cargo clippy (sweep-cli with the testutil feature)"
cargo clippy -p sweep-cli --all-targets --features testutil -- -D warnings

step "cargo test (workspace)"
cargo test --workspace

if [ "$(id -u)" = 0 ]; then
  step "cargo test (sweep-core, ignored: real tmpfs / ext4 loop mounts; root only, skips itself when mounting is not allowed)"
  cargo test -p sweep-core -- --ignored tmpfs ext4_
else
  step "skipping the mount-based wiper / disk analyzer tests (they need root)"
fi

# Real dpkg/apt tests: install and remove a dummy package, so root on a Debian-family system only.
if [ "$(id -u)" = 0 ] && command -v dpkg >/dev/null 2>&1; then
  step "cargo test (real dpkg/apt integration tests: --ignored real_)"
  cargo test -p sweep-core -- --ignored real_ --skip real_chromium
else
  echo "(skipping the real dpkg/apt tests: needs root and dpkg)"
fi

# Real Chromium profile with an unpacked extension (browser plugins). Needs the pre-installed
# Chromium under /opt/pw-browsers; runs it headless and lists the resulting profile.
if ls /opt/pw-browsers/chromium-*/chrome-linux/chrome >/dev/null 2>&1; then
  step "cargo test (real Chromium profile: --ignored real_chromium)"
  cargo test -p sweep-core --lib -- --ignored real_chromium
else
  echo "(skipping the real Chromium profile test: no Chromium under /opt/pw-browsers)"
fi

step "cargo test (sweep-cli with the testutil feature: dev-fixture)"
cargo test -p sweep-cli --features testutil

step "cargo check sweep-core for Windows (x86_64-pc-windows-gnu)"
cargo check -p sweep-core --target x86_64-pc-windows-gnu

step "cargo check sweep-core for macOS (aarch64-apple-darwin)"
cargo check -p sweep-core --target aarch64-apple-darwin

step "pnpm install --frozen-lockfile"
pnpm install --frozen-lockfile

step "pnpm typecheck"
pnpm typecheck

step "pnpm lint"
pnpm lint

step "contrast check of the design tokens (both themes)"
node scripts/check-contrast.mjs

step "pnpm test (vitest)"
pnpm test

step "pnpm build"
pnpm build

step "cargo build -p sweep-cli --features testutil (e2e binary with the dev-fixture command)"
cargo build -p sweep-cli --features testutil

if [[ "${SKIP_E2E:-0}" == "1" ]]; then
  step "pnpm e2e (skipped: SKIP_E2E=1)"
else
  step "pnpm e2e (playwright)"
  pnpm e2e
fi

if [[ "${SKIP_DESKTOP:-0}" == "1" ]]; then
  step "desktop smoke test (skipped: SKIP_DESKTOP=1)"
elif command -v WebKitWebDriver >/dev/null 2>&1 && command -v xvfb-run >/dev/null 2>&1; then
  # The embedded frontend is compiled in (custom-protocol), so this must come after `pnpm build`.
  step "cargo build -p clearsweep-desktop --features custom-protocol"
  CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}" cargo build -p clearsweep-desktop --features custom-protocol
  step "desktop smoke test (real WebKit webview under xvfb: every page, IPC progress + cancel, browser fallback)"
  xvfb-run -a python3 scripts/desktop-smoke.py target/debug/clearsweep-desktop target/debug/clearsweep
else
  echo "(skipping the desktop smoke test: WebKitWebDriver and/or xvfb-run not installed)"
fi

printf '\n\033[1;32mAll checks passed.\033[0m\n'
