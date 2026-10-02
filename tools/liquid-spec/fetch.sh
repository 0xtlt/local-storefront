#!/usr/bin/env bash
# Downloads Shopify's liquid-spec (MIT, https://github.com/Shopify/liquid-spec) into .cache/
# (gitignored), at the revision recorded in tools/liquid-spec/REVISION:
#   .cache/liquid-spec         the suite
#   .cache/liquid-spec-cases   its specs as JSON, one file per suite (see export.rb)
# The JSON files are what crates/liquid/tests/liquid_spec.rs runs.
#
# To move to a newer liquid-spec, put its commit in REVISION, run this again and update the
# baselines of the test.
set -euo pipefail
cd "$(dirname "$0")/../.."

revision="$(tr -d '[:space:]' < tools/liquid-spec/REVISION)"
if [ ! -d .cache/liquid-spec/.git ]; then
  mkdir -p .cache
  git init --quiet .cache/liquid-spec
  git -C .cache/liquid-spec remote add origin https://github.com/Shopify/liquid-spec.git
fi
if [ "$(git -C .cache/liquid-spec rev-parse --quiet --verify HEAD || true)" != "$revision" ]; then
  git -C .cache/liquid-spec fetch --quiet --depth 1 origin "$revision"
  git -C .cache/liquid-spec checkout --quiet --detach FETCH_HEAD
fi
ruby tools/liquid-spec/export.rb
