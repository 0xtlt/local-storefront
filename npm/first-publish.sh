#!/usr/bin/env bash
# Publishes the npm packages of a release from this machine, then makes the npm.yml workflow
# their trusted publisher, so that every later release is published by GitHub Actions without
# a token.
#
# npm can only trust a workflow for a package that already exists, hence this script: run it
# once at the beginning, and again whenever a package is added (a new platform).
#
#   npm login
#   npm/first-publish.sh v0.1.1            # or: mise run npm:first-publish v0.1.1
#   npm/first-publish.sh v0.1.1 --dry-run  # shows what would be published, changes nothing
#
# Needs npm 11.15 or later, two-factor authentication on the npm account, and the GitHub CLI.
# Packages that are already published, or already trusted, are left as they are.

set -euo pipefail

tag=${1:-}
dry_run=${2:-}
if [[ ! $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+ ]] || [[ -n $dry_run && $dry_run != --dry-run ]]; then
  echo "usage: npm/first-publish.sh <tag> [--dry-run]   (e.g. v0.1.1)" >&2
  exit 2
fi
version=${tag#v}

cd "$(dirname "$0")/.."
repository=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
workflow=npm.yml

if [[ -z $dry_run ]]; then
  user=$(npm whoami --registry https://registry.npmjs.org 2>/dev/null) || {
    echo "Not logged in to npm: run \`npm login\` first." >&2
    exit 1
  }
  echo "Publishing as $user."
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

echo "Downloading the archives of $tag from $repository..."
gh release download "$tag" --repo "$repository" --dir "$work/dist" --pattern 'lsf-*'
node npm/build.mjs --artifacts "$work/dist" --out "$work/npm-dist" --version "$version" > /dev/null

# The platform packages first: the launcher depends on them.
packages=()
for directory in "$work"/npm-dist/local-storefront-*/ "$work"/npm-dist/local-storefront/; do
  name=$(basename "$directory")
  packages+=("$name")
  if npm view "$name@$version" version --registry https://registry.npmjs.org > /dev/null 2>&1; then
    echo "$name@$version is already published"
  else
    echo "Publishing $name@$version"
    npm publish "$directory" --access public --registry https://registry.npmjs.org ${dry_run:+--dry-run} > /dev/null
  fi
done

if [[ -n $dry_run ]]; then
  echo "Dry run: nothing was published, and no trusted publisher was configured."
  exit 0
fi

echo
echo "Making $repository ($workflow) the trusted publisher of each package."
echo "npm asks for two-factor authentication once: tick the option to skip it for 5 minutes."
failed=()
for name in "${packages[@]}"; do
  if npm trust list "$name" --json 2> /dev/null | grep -q "\"$workflow\""; then
    echo "$name already trusts $workflow"
  elif npm trust github "$name" --file "$workflow" --repository "$repository" --allow-publish --yes; then
    echo "$name now trusts $workflow"
  else
    failed+=("$name")
  fi
  # npm limits the rate of these calls.
  sleep 2
done

if ((${#failed[@]})); then
  echo
  echo "Could not configure: ${failed[*]}" >&2
  echo "Run the script again, or set the trusted publisher by hand in the settings of the package on npmjs.com" >&2
  echo "(GitHub Actions, repository $repository, workflow $workflow)." >&2
  exit 1
fi
echo
echo "Done. Releases are now published to npm by GitHub Actions."
