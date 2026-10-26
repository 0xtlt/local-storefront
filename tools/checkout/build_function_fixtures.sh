#!/usr/bin/env bash
# Rebuilds the Wasm modules of the fixture app of lsf-checkout
# (crates/checkout/tests/fixtures/app). The modules are committed: this is only needed after
# changing a Function. Needs network access the first time, for Shopify's Function API
# schemas (not committed), the crates and Shopify's trampoline.
#
# A Rust Function built for `wasm32-unknown-unknown` calls the Function Wasm API of the host by
# name. Shopify's CLI links it to the host's provider with `shopify-function-trampoline` when it
# builds an app, and function-runner (what lsf runs Functions with) only runs trampolined
# modules, so the modules in dist/ are trampolined the same way.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
app="$root/crates/checkout/tests/fixtures/app"

# Shopify's examples at a fixed commit hold the schemas of the Function APIs.
examples="19ccafceda1d0052c2c90c0a1e4db3fe37b16c27"
raw="https://raw.githubusercontent.com/Shopify/function-examples/$examples"

# extension handle -> where its schema is
schemas=(
  "volume-discount discounts/rust/discount/default"
  "delivery-rules checkout/rust/delivery-customization/default"
  "payment-rules checkout/rust/payment-customization/default"
  "quantity-limit checkout/rust/cart-checkout-validation/default"
  "bundle-transform checkout/rust/cart-transform/default"
)
for entry in "${schemas[@]}"; do
  read -r handle path <<<"$entry"
  schema="$app/extensions/$handle/schema.graphql"
  [ -s "$schema" ] || curl -fsSL "$raw/$path/schema.graphql" -o "$schema"
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

for entry in "${schemas[@]}"; do
  read -r handle _ <<<"$entry"
  module="${handle//-/_}.wasm"
  mkdir -p "$app/extensions/$handle/dist"
  "$trampoline" -i "$app/target/wasm32-unknown-unknown/release/$module" -o "$app/extensions/$handle/dist/function.wasm"
done
ls -l "$app"/extensions/*/dist/function.wasm
