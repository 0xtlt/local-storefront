#!/usr/bin/env bash
# Rebuilds the Wasm modules of the fixture app of lsf-checkout
# (crates/checkout/tests/fixtures/app). The modules are committed: this is only needed after
# changing a Function. Needs network access the first time, for the crates and Shopify's
# trampoline.
#
# A Rust Function built for `wasm32-unknown-unknown` calls the Function Wasm API of the host by
# name. Shopify's CLI links it to the host's provider with `shopify-function-trampoline` when it
# builds an app, and function-runner (what lsf runs Functions with) only runs trampolined
# modules, so the modules in dist/ are trampolined the same way.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
app="$root/crates/checkout/tests/fixtures/app"

# The schema of each Function is the one of its API version (`api_version` in its
# shopify.extension.toml) and Function API, from crates/checkout/schemas (`mise run
# checkout:schemas`). Not committed here: they are copies, git-ignored.
# extension handle -> Function API
apis=(
  "volume-discount discount"
  "delivery-rules delivery_customization"
  "payment-rules payment_customization"
  "quantity-limit cart_checkout_validation"
  "bundle-transform cart_transform"
)
schemas="$root/crates/checkout/schemas"
for entry in "${apis[@]}"; do
  read -r handle api <<<"$entry"
  version="$(sed -n 's/^api_version = "\(.*\)"/\1/p' "$app/extensions/$handle/shopify.extension.toml" | head -n 1)"
  cp "$schemas/$version/$api.graphql" "$app/extensions/$handle/schema.graphql"
done

trampoline_version="2.0.0"
case "$(uname -s)" in
  Darwin) os="macos" ;;
  Linux) os="linux" ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch="arm" ;;
  x86_64) arch="x86_64" ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac
trampoline="$app/target/shopify-function-trampoline-$trampoline_version"
if [ ! -x "$trampoline" ]; then
  mkdir -p "$app/target"
  curl -fsSL "https://github.com/Shopify/shopify-function-wasm-api/releases/download/shopify_function_trampoline/v$trampoline_version/shopify-function-trampoline-$arch-$os-v$trampoline_version.gz" \
    | gunzip >"$trampoline"
  chmod +x "$trampoline"
fi

rustup target add wasm32-unknown-unknown >/dev/null
cargo build --manifest-path "$app/Cargo.toml" --release --target wasm32-unknown-unknown

for entry in "${apis[@]}"; do
  read -r handle _ <<<"$entry"
  module="${handle//-/_}.wasm"
  mkdir -p "$app/extensions/$handle/dist"
  "$trampoline" -i "$app/target/wasm32-unknown-unknown/release/$module" -o "$app/extensions/$handle/dist/function.wasm"
done
ls -l "$app"/extensions/*/dist/function.wasm
