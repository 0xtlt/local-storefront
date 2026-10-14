#!/usr/bin/env bash
# Downloads the tests of Shopify's Prettier plugin for Liquid (MIT,
# https://github.com/Shopify/theme-tools) into .cache/ (gitignored), at the revision recorded
# in tools/prettier/FIXTURES_REVISION, and has Prettier format each of them:
#   .cache/theme-tools                          the repository
#   .cache/prettier-plugin-liquid-cases.json    its cases, with what Prettier makes of them
# The JSON file is what crates/fmt/tests/plugin_fixtures.rs runs.
#
# To move to a newer plugin, put its version in tools/prettier/package.json and the commit of
# its tests in FIXTURES_REVISION, run `mise run prettier:install` and this again, then update
# the baseline of the test.
set -euo pipefail
cd "$(dirname "$0")/../.."

revision="$(tr -d '[:space:]' < tools/prettier/FIXTURES_REVISION)"
if [ ! -d .cache/theme-tools/.git ]; then
  mkdir -p .cache
  git init --quiet .cache/theme-tools
  git -C .cache/theme-tools remote add origin https://github.com/Shopify/theme-tools.git
fi
if [ "$(git -C .cache/theme-tools rev-parse --quiet --verify HEAD || true)" != "$revision" ]; then
  git -C .cache/theme-tools fetch --quiet --depth 1 origin "$revision"
  git -C .cache/theme-tools checkout --quiet --detach FETCH_HEAD
fi
node tools/prettier/export_fixtures.mjs
