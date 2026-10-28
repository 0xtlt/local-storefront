#!/usr/bin/env bash
# Rebuilds the Wasm modules of the fixture app of lsf-checkout
# (crates/checkout/tests/fixtures/app): see build_functions.sh. The modules are committed: this
# is only needed after changing a Function.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
exec "$here/build_functions.sh" "$here/../../crates/checkout/tests/fixtures/app" "$@"
