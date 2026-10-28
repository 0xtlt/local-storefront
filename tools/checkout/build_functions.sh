#!/usr/bin/env bash
# Builds the Functions of a Shopify app of this repository into their Wasm modules
# (`extensions/<handle>/dist/function.wasm`), the way `shopify app build` does. Used for the test
# app of lsf-checkout (`mise run fixtures:functions`) and the checkout playground
# (`mise run playground:functions`). The modules are committed: this is only needed after
# changing a Function. Needs network access the first time, for the crates, Shopify's trampoline,
# and the npm packages and Shopify CLI of the JavaScript Functions.
#
#   tools/checkout/build_functions.sh <app dir> [handle...]
#
# Without a handle, every Function of the app that has a Cargo.toml (Rust) or a package.json
# (JavaScript or TypeScript) is built. The app is a Cargo workspace of its own whose members are
# the Rust Functions, each package named after its handle.
#
# A Rust Function built for `wasm32-unknown-unknown` calls the Function Wasm API of the host by
# name. Shopify's CLI links it to the host's provider with `shopify-function-trampoline` when it
# builds an app, and function-runner (what lsf runs Functions with) only runs trampolined
# modules, so the modules in dist/ are trampolined the same way.
#
# A JavaScript Function is built by Shopify's CLI (`shopify app function build`: the GraphQL
# typegen, esbuild, then Javy), in a copy of the Function inside a minimal app of its own: the CLI
# checks every extension of the app it builds in, and the checkout UI extensions of the playground
# are not in Shopify's format. In the copy, `[extensions.build] command` is emptied, as in a
# Function that `shopify app generate extension` makes, so that the CLI builds it instead of
# running this script. Only the module is copied back: what else the build writes (`generated/`)
# would start another build in watch mode.
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "usage: $0 <app dir> [handle...]" >&2
  exit 2
fi
root="$(cd "$(dirname "$0")/../.." && pwd)"
app="$(cd "$1" && pwd)"
shift
schemas="$root/crates/checkout/schemas"
# The Shopify CLI that builds the JavaScript Functions (`npx` downloads it the first time).
shopify_cli_version="4.9.5"

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

# The JavaScript Functions, each in a minimal app of its own (see above).
for handle in ${javascript[@]+"${javascript[@]}"}; do
  scratch="$(mktemp -d)"
  cat >"$scratch/shopify.app.toml" <<'TOML'
client_id = "00000000000000000000000000000000"
name = "build"
application_url = "https://example.com"
embedded = true

[access_scopes]
scopes = ""

[auth]
redirect_urls = ["https://example.com/auth/callback"]

[webhooks]
api_version = "2026-10"
TOML
  copy="$scratch/extensions/$handle"
  mkdir -p "$scratch/extensions"
  cp -R "$app/extensions/$handle" "$copy"
  rm -rf "$copy/node_modules" "$copy/dist" "$copy/generated"
  sed 's/^\( *command = \).*/\1""/' "$app/extensions/$handle/shopify.extension.toml" \
    >"$copy/shopify.extension.toml"
  (
    cd "$copy"
    npm ci --no-audit --no-fund --loglevel=error
    SHOPIFY_CLI_NO_ANALYTICS=1 npx --yes "@shopify/cli@$shopify_cli_version" app function build --path .
  )
  mkdir -p "$app/extensions/$handle/dist"
  cp "$copy/dist/function.wasm" "$app/extensions/$handle/dist/function.wasm"
  rm -rf "$scratch"
done

for handle in "${handles[@]}"; do
  ls -l "$app/extensions/$handle/dist/function.wasm"
done
