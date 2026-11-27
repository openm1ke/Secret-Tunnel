import copy
import importlib.util
import json
import subprocess
import sys
import tempfile
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("hosting_updater", ROOT/"scripts/update-hosting-catalog.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
FIXTURES = Path(__file__).parent/"hosting-fixtures"


class HostingCatalogTests(unittest.TestCase):
    def source(self, provider, url=None):
        return (FIXTURES/f"{provider}.source").read_text()

    def test_official_formats_and_monthly_prices(self):
        expected = {"ovh": (649, 400), "itldc": (399, None), "timeweb": (118000, 1000), "firstvds": (29900, 100),
                    "iphoster": (395, 100), "vultr": (500, None), "hetzner": (649, None)}
        for name, (price, speed) in expected.items():
            with self.subTest(name=name):
                extras = {key: self.source(f"{name}-{key}") for key in module.EXTRA_SOURCES.get(name, {})}
                result = module.ADAPTERS[name](self.source(name), **extras)
                self.assertEqual(result["price_minor"], price)
                self.assertEqual(result["port_mbps"], speed)

    def test_ovh_ignores_setup_and_long_commitment_prices(self):
        data = json.loads(self.source("ovh"))
        prices = data["plans"][0]["pricings"]
        prices.reverse()
        self.assertEqual(module.ovh(json.dumps(data))["price_minor"], 649)
        prices.extend(copy.deepcopy(x) for x in list(prices) if x["price"] == 649000000)
        with self.assertRaises(ValueError):
            module.ovh(json.dumps(data))

    def test_changed_source_preserves_facts_and_original_timestamp(self):
        catalog = json.loads((ROOT/"site/catalog/hosting-v1.json").read_text())
        old = copy.deepcopy(catalog)
        def source(provider, url):
            return "maintenance page" if provider == "firstvds" else self.source(provider)
        updated, failures = module.update(catalog, source, "2026-11-28T00:00:00Z")
        self.assertEqual(catalog, old)
        first = next(x for x in updated["offers"] if x["provider_id"] == "firstvds")
        previous = next(x for x in old["offers"] if x["provider_id"] == "firstvds")
        self.assertEqual(first["price_minor"], previous["price_minor"])
        self.assertEqual(first["verified_at"], previous["verified_at"])
        self.assertTrue(first["update_error"])
        self.assertEqual(len(failures), 1)
        self.assertEqual(updated["offers"][0]["verified_at"], "2026-11-28T00:00:00Z")

    def test_currency_and_free_price_are_rejected(self):
        data = json.loads(self.source("ovh")); data["locale"]["currencyCode"] = "USD"
        with self.assertRaises(ValueError): module.ovh(json.dumps(data))
        for value in [0, -1, "NaN", "Infinity", "1.001"]:
            with self.subTest(value=value), self.assertRaises((ValueError, ArithmeticError)):
                module.cents(value)

    def test_iphoster_monthly_currency_and_ambiguous_card(self):
        raw = self.source("iphoster")
        self.assertEqual(module.iphoster(raw.replace('data-y="3.950000"', 'data-y="1.000000"'))["price_minor"], 395)
        for changed in [raw.replace('data-cur="USD"', 'data-cur="EUR"'), raw.replace("IPv4 + IPv6", "IPv6 only"),
                        raw.replace("</article>", "</article>" + raw[raw.index("<article"):raw.index("</article>")+10])]:
            with self.subTest(changed=changed[:80]), self.assertRaises(ValueError):
                module.iphoster(changed)

    def test_vultr_location_price_and_pagination(self):
        data = json.loads(self.source("vultr"))
        p = next(x for x in data["plans"] if x["id"] == "vc2-1c-1gb")
        p["location_cost"]["ams"] = {"monthly_cost": 6.5}
        self.assertEqual(module.vultr(json.dumps(data))["price_minor"], 650)
        p["locations"].remove("ams")
        with self.assertRaises(ValueError): module.vultr(json.dumps(data))
        data = json.loads(self.source("vultr")); data["meta"]["links"]["next"] = "next-page"
        with self.assertRaises(ValueError): module.vultr(json.dumps(data))

    def test_hetzner_sums_ipv4_and_uses_exact_active_location(self):
        raw, page, ip = (self.source(n) for n in ("hetzner", "hetzner-page", "hetzner-ipv4"))
        self.assertEqual(module.hetzner(raw, page, ip)["price_minor"], 649)
        # The page's fallback price is deliberately different from live API.
        self.assertEqual(module.hetzner(raw, page.replace("5.49", "1.00"), ip)["price_minor"], 649)
        for changed in [raw.replace('"active":true', '"active":false'), raw.replace('"NBG1"', '"HEL1"')]:
            with self.assertRaises(ValueError): module.hetzner(changed, page, ip)
        with self.assertRaises(ValueError): module.hetzner(raw, page.replace("CLOUD_140+CLOUD_21", "CLOUD_999+CLOUD_21"), ip)
        with self.assertRaises((ValueError, KeyError)): module.hetzner(raw, page, ip.replace('"EUR":"0.50"', '"USD":"0.50"'))

    def test_published_baseline_does_not_roll_back_prices_or_control_targets(self):
        seed = module.load(ROOT/"site/catalog/hosting-v1.json")
        published = copy.deepcopy(seed)
        published["generated_at"] = "2026-11-28T00:00:00Z"
        p = published["offers"][0]
        p.update(price_minor=1234, verified_at=published["generated_at"], test_host="different.example.com")
        merged = module.baseline(seed, published)
        self.assertEqual(merged["offers"][0]["price_minor"], 1234)
        self.assertEqual(merged["offers"][0]["test_host"], seed["offers"][0]["test_host"])
        def source(provider, url): raise OSError("offline")
        value, errors = module.update(merged, source, "2026-11-29T00:00:00Z")
        self.assertEqual(value["offers"][0]["price_minor"], 1234)
        self.assertEqual(value["offers"][0]["verified_at"], published["generated_at"])
        self.assertEqual(len(errors), len(seed["offers"]))
        self.assertEqual(published["offers"][0]["test_host"], "different.example.com")

    def test_bad_adapter_facts_preserve_entire_previous_offer(self):
        catalog = module.load(ROOT/"site/catalog/hosting-v1.json")
        from unittest.mock import patch
        with patch.dict(module.ADAPTERS, {"ovh": lambda _: dict(cpu=0)}):
            result, errors = module.update(catalog, self.source, "2026-11-28T00:00:00Z")
        self.assertEqual(len(errors), 1)
        previous = dict(catalog["offers"][0]); previous["update_error"] = result["offers"][0]["update_error"]
        self.assertEqual(result["offers"][0], previous)

    def test_malformed_or_oversized_catalog_is_rejected(self):
        catalog = module.load(ROOT/"site/catalog/hosting-v1.json")
        for changes in [dict(test_host="127.0.0.1"), dict(test_host="198.18.0.1"), dict(test_host="64:ff9b::101:101"),
                        dict(source_url="http://provider.example"), dict(test_port=22), dict(cpu=True), dict(price_minor=0)]:
            value = copy.deepcopy(catalog); value["offers"][0].update(changes)
            with self.subTest(changes=changes), self.assertRaises(ValueError): module.validate(value)
        with tempfile.TemporaryDirectory() as work:
            path = Path(work)/"bad.json"; path.write_bytes(b" "*(module.MAX_CATALOG_BYTES+1))
            with self.assertRaises(ValueError): module.load(path)

    def test_cli_reports_partial_and_total_failure_and_refuses_bad_baseline(self):
        import shutil
        with tempfile.TemporaryDirectory() as work:
            work = Path(work); sources = work/"sources"; shutil.copytree(FIXTURES, sources)
            (sources/"firstvds.source").write_text("maintenance")
            output, report = work/"candidate.json", work/"report.json"
            cmd = [sys.executable, str(ROOT/"scripts/update-hosting-catalog.py"), "--source-dir", str(sources),
                   "--output", str(output), "--report", str(report), "--allow-partial"]
            run = subprocess.run(cmd, capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertEqual(json.loads(report.read_text())["status"], "partial")
            self.assertEqual(len(module.load(output)["offers"]), 7)
            for path in sources.glob("*.source"): path.write_text("maintenance")
            run = subprocess.run(cmd, capture_output=True, text=True)
            self.assertEqual(run.returncode, 1, run.stderr)
            self.assertEqual(json.loads(report.read_text())["status"], "failed")
            previous = output.read_bytes()
            baseline = work/"published.json"; baseline.write_text("broken json")
            run = subprocess.run(cmd+["--published", str(baseline)], capture_output=True, text=True)
            self.assertNotEqual(run.returncode, 0)
            self.assertEqual(output.read_bytes(), previous)


if __name__ == "__main__":
    unittest.main()
