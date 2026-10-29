#!/usr/bin/env bash
# Rebuilds crates/checkout/providers from the providers of Shopify's function-runner at the tag
# below: the Wasm modules a Function's module imports and lsf links when it runs one (Shopify's
# Javy plugins, the providers of the Function Wasm API). They are 9.7 MB, mostly the same QuickJS
# again and again: one Brotli stream of all of them, with a 16 MB window, is 1.6 MB.
#
# providers.txt lists them in the order of the stream: name, size, SHA-256. The ones the current
# Functions import come first: lsf reads the stream up to the end of the provider it needs. Takes
# git and brotli (`brew install brotli`).
set -euo pipefail
tag=v9.2.2
here="$(cd "$(dirname "$0")" && pwd)"
out="$here/../../crates/checkout/providers"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$tag" https://github.com/Shopify/function-runner "$work/function-runner"

# What Rust Functions (the Function Wasm API) and JavaScript Functions import today.
first=(shopify_function_v2 shopify_functions_javy_v4)
files=()
for name in "${first[@]}"; do
  files+=("$work/function-runner/providers/$name.wasm")
done
for file in "$work"/function-runner/providers/*.wasm; do
  case " ${first[*]} " in
    *" $(basename "$file" .wasm) "*) ;;
    *) files+=("$file") ;;
  esac
done

mkdir -p "$out"
{
  echo "# The providers of Shopify's function-runner $tag (Apache-2.0): name, size, SHA-256."
  for file in "${files[@]}"; do
    name="$(basename "$file" .wasm)"
    size="$(wc -c < "$file" | tr -d ' ')"
    sum="$(shasum -a 256 "$file" | cut -d ' ' -f 1)"
    echo "$name $size $sum"
    cat "$file" >> "$work/providers"
  done
} > "$out/providers.txt"
brotli --quality=11 --lgwin=24 --force --output="$out/providers.br" "$work/providers"
