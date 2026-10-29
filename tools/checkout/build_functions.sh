#!/usr/bin/env bash
# Builds the Functions of a Shopify app of this repository into their Wasm modules
# (`extensions/<handle>/dist/function.wasm`), the way `shopify app build` does. Used for the test
# app of lsf-checkout (`mise run fixtures:functions`) and the checkout playground
# (`mise run playground:functions`). The modules are committed: this is only needed after
# changing a Function. Needs network access the first time, for the crates and Shopify's
# trampoline.
#
#   tools/checkout/build_functions.sh <app dir> [handle...]
#
# Without a handle, every Function of the app that has a Cargo.toml (Rust) or a package.json
# (JavaScript or TypeScript) is built. The app is a Cargo workspace of its own whose members are
# the Rust Functions, each package named after its handle.
#
# A Rust Function built for `wasm32-unknown-unknown` calls the Function Wasm API of the host by
# name. Shopify's CLI links it to the host's provider with `shopify-function-trampoline` when it
# builds an app, and function-runner (which lsf runs Functions as) only runs trampolined
# modules, so the modules in dist/ are trampolined the same way.
#
# A JavaScript Function is built by lsf (`lsf function build`), as Shopify's CLI builds it, without
# Node.
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "usage: $0 <app dir> [handle...]" >&2
  exit 2
fi
root="$(cd "$(dirname "$0")/../.." && pwd)"
app="$(cd "$1" && pwd)"
shift
schemas="$root/crates/checkout/schemas"

# The schema of a Function API at a release: the file of the newest release at or before it
# (`mise run checkout:schemas` writes one only when a release changes the schema). A release older
# than every file (a Function made to show that it is refused) is built with the newest one.
schema_of() {
  local found="" newest="" file release
  for file in "$schemas"/*/"$1.graphql"; do
    newest="$file"
    release="$(basename "$(dirname "$file")")"
    [[ "$release" > "$2" ]] || found="$file"
  done
  echo "${found:-$newest}"
}

# The Function API of a target: the name of its schema in crates/checkout/schemas/<release>.
# Empty for the APIs lsf has no schema for (they are listed, not run): such a Function is written
# without the typegen and needs none.
api_of() {
  case "$1" in
    cart.transform.run | purchase.cart-transform.run) echo cart_transform ;;
    cart.lines.discounts.generate.* | cart.delivery-options.discounts.generate.*) echo discount ;;
    purchase.product-discount.run) echo product_discount ;;
    purchase.order-discount.run) echo order_discount ;;
    purchase.shipping-discount.run) echo shipping_discount ;;
    cart.delivery-options.transform.run | purchase.delivery-customization.run) echo delivery_customization ;;
    cart.payment-methods.transform.run | purchase.payment-customization.run) echo payment_customization ;;
    cart.validations.generate.* | purchase.validation.run) echo cart_checkout_validation ;;
    *) echo "" ;;
  esac
}

if [ $# -gt 0 ]; then
  handles=("$@")
else
  handles=()
  for toml in "$app"/extensions/*/shopify.extension.toml; do
    dir="$(dirname "$toml")"
    if { [ -f "$dir/Cargo.toml" ] || [ -f "$dir/package.json" ]; } \
      && grep -q '^type = "function"' "$toml"; then
      handles+=("$(basename "$dir")")
    fi
  done
fi

# The schema of each Function is the one of its API version (`api_version` in its
# shopify.extension.toml) and Function API, from crates/checkout/schemas (`schema_of`). Not
# committed with the app: they are copies, git-ignored.
rust=()
javascript=()
for handle in "${handles[@]}"; do
  toml="$app/extensions/$handle/shopify.extension.toml"
  if [ -f "$app/extensions/$handle/Cargo.toml" ]; then
    rust+=("$handle")
  elif [ -f "$app/extensions/$handle/package.json" ]; then
    javascript+=("$handle")
  else
    echo "$handle: not a Rust or JavaScript Function of $app" >&2
    exit 1
  fi
  version="$(sed -n 's/^api_version = "\(.*\)"/\1/p' "$toml" | head -n 1)"
  target="$(sed -n 's/^ *target = "\(.*\)"/\1/p' "$toml" | head -n 1)"
  api="$(api_of "$target")"
  if [ -n "$api" ]; then
    cp "$(schema_of "$api" "$version")" "$app/extensions/$handle/schema.graphql"
  fi
done

if [ ${#rust[@]} -gt 0 ]; then
  rustup target add wasm32-unknown-unknown >/dev/null
  packages=()
  for handle in "${rust[@]}"; do
    packages+=(-p "$handle")
  done
  cargo build --manifest-path "$app/Cargo.toml" --release --target wasm32-unknown-unknown "${packages[@]}"

  # Shopify's trampoline, next to what cargo built (after the build: cargo makes target/, which may
  # be a link to a cache folder that is gone).
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
      | gunzip >"$trampoline.part"
    chmod +x "$trampoline.part"
    mv "$trampoline.part" "$trampoline"
  fi

  for handle in "${rust[@]}"; do
    module="${handle//-/_}.wasm"
    mkdir -p "$app/extensions/$handle/dist"
    rm -f "$app/extensions/$handle/dist/function.wasm"
    "$trampoline" -i "$app/target/wasm32-unknown-unknown/release/$module" -o "$app/extensions/$handle/dist/function.wasm"
  done
fi

if [ ${#javascript[@]} -gt 0 ]; then
  cargo run --quiet --release --manifest-path "$root/Cargo.toml" -p lsf -- \
    function build --app "$app" "${javascript[@]}"
fi

for handle in "${handles[@]}"; do
  ls -l "$app/extensions/$handle/dist/function.wasm"
done
