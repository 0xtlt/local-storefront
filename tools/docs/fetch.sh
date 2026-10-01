#!/usr/bin/env bash
# Downloads Shopify's Liquid reference into .cache/ (gitignored):
#   .cache/theme-liquid-docs   machine-readable list of objects, filters and tags (MIT)
#   .cache/shopify-docs        one markdown page per filter/tag/object, with example outputs
# The pages are the ground truth for the conformance tests in crates/core/tests/conformance.rs.
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ -d .cache/theme-liquid-docs/.git ]; then
  git -C .cache/theme-liquid-docs pull --ff-only --quiet
else
  git clone --depth 1 --quiet https://github.com/Shopify/theme-liquid-docs.git .cache/theme-liquid-docs
fi

names() {
  ruby -rjson -e 'puts JSON.parse(File.read(ARGV[0], encoding: "UTF-8")).map { |e| e["name"] }.uniq' ".cache/theme-liquid-docs/data/$1.json"
}

fetch() {
  local kind="$1" name="$2"
  local out=".cache/shopify-docs/$kind/$name.md"
  [ -s "$out" ] && return 0
  curl -sS --fail -m 30 -A "shopify-local-theme docs fetch" -o "$out" "https://shopify.dev/docs/api/liquid/$kind/$name.md" || echo "failed: $kind/$name" >&2
}
export -f fetch

for kind in filters tags objects; do
  mkdir -p ".cache/shopify-docs/$kind"
  names "$kind" | xargs -P 6 -I{} bash -c 'fetch "$@"' _ "$kind" {}
done
echo "filters: $(ls .cache/shopify-docs/filters | wc -l), tags: $(ls .cache/shopify-docs/tags | wc -l), objects: $(ls .cache/shopify-docs/objects | wc -l)"
