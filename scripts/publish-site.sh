#!/bin/sh
# Publish site/ to the gh-pages branch, which GitHub Pages serves at
# https://openm1ke.github.io/Secret-Tunnel/. Keep the latest catalog produced
# by Actions; a site checkout may contain older prices. The releases page reads GitHub's
# releases in the browser, so new releases need no publish.
set -eu
root=$(git rev-parse --show-toplevel)
remote=$(git -C "$root" remote get-url origin)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
git clone -q --depth=1 --branch gh-pages --single-branch "$remote" "$work"
# A fast-forward push refuses a concurrent publication instead of overwriting it.
rsync -a --delete --exclude='.git/' --exclude='/catalog/hosting-v1.json' "$root/site/" "$work/"
python3 - "$root" "$work" <<'PY'
import importlib.util
from pathlib import Path
import sys
root, work = map(Path, sys.argv[1:])
spec = importlib.util.spec_from_file_location("hosting_updater", root/"scripts/update-hosting-catalog.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
bundled_path = root/"site/catalog/hosting-v1.json"
published_path = work/"catalog/hosting-v1.json"
bundled = module.load(bundled_path) if bundled_path.exists() else None
published = module.load(published_path) if published_path.exists() else None
if bundled or published:
    latest = max((c for c in (bundled, published) if c), key=lambda c: module.timestamp(c["generated_at"]))
    module.write_json(published_path, latest)
PY
# The setup guide for AI assistants is served next to llms.txt; docs/ stays its only source.
cp "$root/docs/SETUP-FOR-AI.md" "$work/setup-for-ai.md"
touch "$work/.nojekyll"
cd "$work"
git add -A
if ! git diff --cached --quiet; then
  git -c user.name="$(git -C "$root" config user.name)" -c user.email="$(git -C "$root" config user.email)" \
    commit -q -m "Site from $(git -C "$root" rev-parse --short HEAD)"
  git push -q origin HEAD:gh-pages
fi
echo "Published site/ to gh-pages"
