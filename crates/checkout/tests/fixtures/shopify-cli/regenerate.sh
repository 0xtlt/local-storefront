#!/usr/bin/env bash
# Builds the playground's extensions written in JavaScript and TypeScript with Shopify's CLI, as
# `shopify app deploy` does (from the root of an app), and copies what it wrote here: the modules
# of the Functions and the bundles of the checkout UI extensions lsf's builds are compared with
# (crates/checkout/src/javascript/tests.rs, crates/checkout/assets/tests/built-extensions.test.mjs).
# Needs Node, npm and the network; nothing else in this repository does.
#
#   crates/checkout/tests/fixtures/shopify-cli/regenerate.sh [cli version]
set -euo pipefail

version="${1:-5.0.0}"
root="$(cd "$(dirname "$0")/../../../../.." && pwd)"
here="$root/crates/checkout/tests/fixtures/shopify-cli"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

# A minimal app of their own: Shopify's CLI checks every extension of the app it builds in, and
# the playground's checkout UI extensions are not in Shopify's format.
mkdir -p "$scratch/app/extensions"
cat >"$scratch/app/shopify.app.toml" <<'TOML'
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
for pair in discount-typescript:discount validation-javascript:cart_checkout_validation; do
  handle="${pair%%:*}"
  api="${pair##*:}"
  source="$root/examples/checkout-playground/extensions/$handle"
  target="$scratch/app/extensions/$handle"
  mkdir -p "$target"
  cp -R "$source/src" "$source/package.json" "$source/shopify.extension.toml" "$target/"
  cp "$root/crates/checkout/schemas/2026-10/$api.graphql" "$target/schema.graphql"
  (cd "$target" && npm install --no-audit --no-fund --loglevel=error)
  (cd "$scratch/app" && SHOPIFY_CLI_NO_ANALYTICS=1 npx --yes "@shopify/cli@$version" \
    app function build --path "extensions/$handle")
  cp "$target/dist/function.wasm" "$here/$handle.wasm"
done
for handle in ui-preact ui-preact-typescript; do
  source="$root/examples/checkout-playground/extensions/$handle"
  target="$scratch/app/extensions/$handle"
  mkdir -p "$target"
  cp -R "$source/src" "$source/package.json" "$source/tsconfig.json" "$source/shopify.extension.toml" "$target/"
  [ -d "$source/locales" ] && cp -R "$source/locales" "$target/"
  (cd "$target" && npm install --no-audit --no-fund --loglevel=error)
done
(cd "$scratch/app" && SHOPIFY_CLI_NO_ANALYTICS=1 npx --yes "@shopify/cli@$version" app build --skip-dependencies-installation)
for handle in ui-preact ui-preact-typescript; do
  cp "$scratch/app/extensions/$handle/dist/$handle.js" "$here/$handle.js"
done
ls -l "$here"/*.wasm "$here"/*.js
