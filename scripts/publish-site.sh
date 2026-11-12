#!/bin/sh
# Publish site/ to the gh-pages branch, which GitHub Pages serves at
# https://openm1ke.github.io/Secret-Tunnel/. The branch holds only the built
# site and is replaced on every publish. The releases page reads GitHub's
# releases in the browser, so new releases need no publish.
set -eu
root=$(git rev-parse --show-toplevel)
remote=$(git -C "$root" remote get-url origin)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp -R "$root/site/." "$work/"
touch "$work/.nojekyll"
cd "$work"
git init -q -b gh-pages
git add -A
git -c user.name="$(git -C "$root" config user.name)" -c user.email="$(git -C "$root" config user.email)" \
  commit -q -m "Site from $(git -C "$root" rev-parse --short HEAD)"
git push -q --force "$remote" gh-pages
echo "Published site/ to gh-pages"
