#!/usr/bin/env bash
#
# Run the whole suite the way CI would, in the order that fails fastest.
#
#   ./test.sh            use the current session's display
#   ./test.sh --headless run under Xvfb and a private D-Bus session
#
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# GTK_A11Y=none skips the accessibility bus, a common source of CI hangs.
# GSETTINGS_BACKEND=memory keeps tests from touching real user state.
export GTK_A11Y=none
export GSETTINGS_BACKEND=memory
export RUST_BACKTRACE=1

# Widget tests need a display; the model tests do not, and are the bulk.
run=(cargo test --all-targets)
if [[ "${1:-}" == "--headless" ]]; then
  command -v xvfb-run >/dev/null || { echo "install xvfb first" >&2; exit 1; }
  run=(xvfb-run -a dbus-run-session -- cargo test --all-targets)
fi

echo "==> cargo fmt --check"
cargo fmt --check

echo "==> cargo clippy"
cargo clippy --all-targets -- -D warnings

echo "==> ${run[*]}"
"${run[@]}"

echo
echo "All checks passed."
