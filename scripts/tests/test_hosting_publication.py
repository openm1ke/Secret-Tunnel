"""Exercise the real site publisher against a local bare origin, never GitHub."""
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class HostingPublicationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.work = Path(self.temp.name)
        self.origin = self.work/"origin.git"
        self.repo = self.work/"repo"
        self.pages = self.work/"pages"
        self.git(self.work, "init", "--bare", str(self.origin))
        for repo in (self.repo, self.pages):
            repo.mkdir()
            self.git(repo, "init", "-b", "main" if repo == self.repo else "gh-pages")
            self.git(repo, "config", "user.name", "Catalog test")
            self.git(repo, "config", "user.email", "catalog@example.com")
            self.git(repo, "remote", "add", "origin", str(self.origin))
        for directory in ("scripts", "docs", "site/catalog"):
            (self.repo/directory).mkdir(parents=True, exist_ok=True)
        for name in ("publish-site.sh", "update-hosting-catalog.py"):
            shutil.copy2(ROOT/"scripts"/name, self.repo/"scripts"/name)
        shutil.copy2(ROOT/"site/catalog/hosting-v1.json", self.repo/"site/catalog/hosting-v1.json")
        (self.repo/"docs/SETUP-FOR-AI.md").write_text("setup guide\n")
        (self.repo/"site/index.html").write_text("new site\n")
        self.git(self.repo, "add", ".")
        self.git(self.repo, "commit", "-m", "Source")
        (self.pages/"catalog").mkdir()
        catalog = json.loads((ROOT/"site/catalog/hosting-v1.json").read_text())
        catalog["generated_at"] = "2026-11-28T00:00:00Z"
        catalog["offers"][0].update(price_minor=1234, verified_at=catalog["generated_at"])
        self.latest = json.dumps(catalog, ensure_ascii=False, indent=2)+"\n"
        (self.pages/"catalog/hosting-v1.json").write_text(self.latest)
        (self.pages/"obsolete.html").write_text("old page")
        self.push_pages()

    def git(self, cwd, *args):
        run = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True)
        self.assertEqual(run.returncode, 0, run.stderr)
        return run.stdout.strip()

    def push_pages(self):
        self.git(self.pages, "add", ".")
        self.git(self.pages, "commit", "-m", "Published data")
        self.git(self.pages, "push", "origin", "HEAD:gh-pages")

    def test_site_publish_preserves_latest_catalog_and_history(self):
        old = self.git(self.origin, "rev-parse", "gh-pages")
        run = subprocess.run(["sh", "scripts/publish-site.sh"], cwd=self.repo, capture_output=True, text=True)
        self.assertEqual(run.returncode, 0, run.stderr)
        new = self.git(self.origin, "rev-parse", "gh-pages")
        self.git(self.origin, "merge-base", "--is-ancestor", old, new)
        actual = self.git(self.origin, "show", "gh-pages:catalog/hosting-v1.json")
        self.assertEqual(json.loads(actual), json.loads(self.latest))
        self.assertEqual(self.git(self.origin, "show", "gh-pages:index.html"), "new site")
        self.assertEqual(self.git(self.origin, "show", "gh-pages:setup-for-ai.md"), "setup guide")
        self.assertNotIn("obsolete.html", self.git(self.origin, "ls-tree", "--name-only", "gh-pages"))

    def test_bad_published_catalog_aborts_without_overwriting_site(self):
        (self.pages/"catalog/hosting-v1.json").write_text("broken JSON")
        self.push_pages()
        before = self.git(self.origin, "rev-parse", "gh-pages")
        run = subprocess.run(["sh", "scripts/publish-site.sh"], cwd=self.repo, capture_output=True, text=True)
        self.assertNotEqual(run.returncode, 0)
        self.assertEqual(self.git(self.origin, "rev-parse", "gh-pages"), before)


if __name__ == "__main__":
    unittest.main()
