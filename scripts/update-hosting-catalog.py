#!/usr/bin/env python3
"""Update public offers without app releases. No login, browser execution or scraping proxies.

An adapter failure retains that offer and its original verified_at. Exit 1 means
at least one source needs review; the partial output is still a valid catalog.
"""
import argparse
import copy
from datetime import datetime, timezone
from decimal import Decimal
import ipaddress
from html.parser import HTMLParser
import json
from pathlib import Path
import re
import sys
from urllib.request import Request, urlopen
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
SOURCES = {
    "ovh": "https://eu.api.ovh.com/1.0/order/catalog/public/vps?ovhSubsidiary=IE",
    "itldc": "https://itldc.com/en/vds/",
    "timeweb": "https://timeweb.cloud/services/vds-vps",
    "firstvds": "https://firstvds.ru/products/vds_vps_hosting",
    "iphoster": "https://iphoster.net/vds",
    "vultr": "https://api.vultr.com/v2/plans?per_page=500",
    "hetzner": "https://website-price-api.hetzner.com/api/v1/products/CLOUD_140?location=NBG1&priceType=monthly",
}
EXTRA_SOURCES = {
    "hetzner": {
        "page": "https://www.hetzner.com/cloud/regular-performance/",
        "ipv4": "https://website-price-api.hetzner.com/api/v1/products/CLOUD_21?location=ALL&priceType=monthly",
    },
}
MAX_CATALOG_BYTES = 262_144
FACTS = ("plan", "price_minor", "currency", "cpu", "ram_mb", "disk_gb", "port_mbps",
         "conditions", "price_note", "verified_at", "source_url", "update_error")


class Embedded(HTMLParser):
    def __init__(self, text):
        super().__init__(convert_charrefs=True)
        self.scripts, self.islands, self.attrs, self.parts = [], [], None, []
        self.feed(text)

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "astro-island" and "props" in attrs:
            self.islands.append(json.loads(attrs["props"]))
        if tag == "script":
            self.attrs, self.parts = attrs, []

    def handle_data(self, text):
        if self.attrs is not None:
            self.parts.append(text)

    def handle_endtag(self, tag):
        if tag == "script" and self.attrs is not None:
            self.scripts.append((self.attrs, "".join(self.parts)))
            self.attrs = None


def one(items):
    items = list(items)
    if len(items) != 1:
        raise ValueError(f"Ожидался один точный тариф, найдено: {len(items)}")
    return items[0]


def cents(value):
    v = Decimal(str(value)) * 100
    if not v.is_finite() or v <= 0 or v > 100_000_000 or v != v.to_integral_value():
        raise ValueError("Некорректная цена")
    return int(v)


def ovh(text):
    data = json.loads(text)
    if data["locale"]["currencyCode"] != "EUR":
        raise ValueError("Изменилась валюта каталога")
    plan = one(x for x in data["plans"] if x["planCode"] == "vps-2025-model1")
    price = one(x for x in plan["pricings"] if x["mode"] == "default"
                and x["phase"] == 1 and x["commitment"] == 0 and x["interval"] == 1
                and x["intervalUnit"] == "month" and "renew" in x["capacities"])
    hw = one(x for x in data["products"] if x["name"] == plan["product"])["blobs"]["technical"]
    return dict(plan=plan["invoiceName"], price_minor=cents(Decimal(price["price"]) / 100_000_000),
                currency="EUR", cpu=hw["cpu"]["cores"], ram_mb=hw["memory"]["size"]*1024,
                disk_gb=hw["storage"]["disks"][0]["capacity"], port_mbps=hw["bandwidth"]["level"],
                price_note="Без НДС · базовая цена", conditions="Базовая цена без НДС, каталог IE. Налоги и обязательные опции уточните при заказе. Трафик без лимита в европейских локациях.")


def astro(value):
    tag, data = value
    if tag == 0:
        return {k: astro(v) for k, v in data.items()} if isinstance(data, dict) else data
    if tag == 1:
        return [astro(v) for v in data]
    raise ValueError(f"Неизвестный тег Astro: {tag}")


def itldc(text):
    props = one(x for x in Embedded(text).islands if "products" in x)
    product = one(x for x in astro(props["products"]) if x["name"] == "AMS NVMe VDS 1G" and x["locationCode"] == "EU1.AMS")
    return dict(plan=product["name"], price_minor=cents(product["priceEur"]), currency="EUR",
                cpu=int(product["cpu"].split()[0]), ram_mb=int(product["ram"].split()[0])*1024,
                disk_gb=int(product["storage"].split()[0]), port_mbps=None,
                price_note="Налоги и IPv4 — у хостера", conditions="Скорость порта не указана в карточке источника. Налоги, IPv4 и лимит трафика уточните перед оплатой.")


def nuxt(text):
    raw = json.loads(one(s for a, s in Embedded(text).scripts if a.get("id") == "__NUXT_DATA__"))
    memo = {}
    def decode(i):
        if not isinstance(i, int) or i < 0:
            return None
        if i in memo:
            return memo[i]
        v = raw[i]
        if isinstance(v, dict):
            out = {}; memo[i] = out
            out.update({k: decode(x) for k, x in v.items()}); return out
        if isinstance(v, list):
            if v and isinstance(v[0], str):
                if v[0] not in ("Reactive", "ShallowReactive", "Ref", "ShallowRef") or len(v) != 2:
                    raise ValueError("Неизвестный тип Nuxt")
                return decode(v[1])
            out = []; memo[i] = out
            out.extend(decode(x) for x in v); return out
        return v
    root = raw[raw[0][1]] if isinstance(raw[0], list) else raw[0]
    data = raw[root["data"]]
    if isinstance(data, list):
        data = raw[data[1]]
    return {"data": {"vds/fetchTariffs": decode(data["vds/fetchTariffs"])}}


def timeweb(text):
    tariffs = nuxt(text)["data"]["vds/fetchTariffs"]
    t = one(x for x in tariffs if x["name"] == "Cloud MSK 15" and "msk_nvme" in x["tags"])
    return dict(plan=t["name"], price_minor=cents(t["price"]), currency="RUB",
                cpu=int(t["cpu"].split()[0]), ram_mb=int(t["memory"].split()[0])*1024,
                disk_gb=int(t["storage"][0]["size"].split()[0]), port_mbps=t["bandwidth"],
                price_note="Базовая цена · IPv4 уточните", conditions="Базовый тариф помесячно. Публичный IPv4, дополнительные услуги и условия трафика проверьте в конфигураторе.")


def firstvds(text):
    data = json.loads(one(s for a, s in Embedded(text).scripts if "data-tariffs" in a))
    t = one(x for x in data["VDS Прогрев"]["tariff"] if x["virtualization_type"] == "KVM" and x["hdd_type"] == "SSD")
    d = t["defaults"]
    traffic = t["addon"]["traffic"]["values"][d["traffic"]]
    speed = re.fullmatch(r"Неограниченный трафик (\d+) Мбит/с", traffic["name"])
    if not speed or Decimal(traffic["price"]["RUB"]["1"]) != 0 or d["ip"] != 1:
        raise ValueError("Изменились условия сети/IPv4")
    return dict(plan=t["name"], price_minor=cents(t["price"]["RUB"]["1"]), currency="RUB",
                cpu=d["cpu"], ram_mb=d["ram"]*1024, disk_gb=d["hdd"], port_mbps=int(speed[1]),
                price_note="IPv4 включён", conditions="Linux, KVM, один IPv4, трафик без лимита до указанной скорости. Без платной панели и дополнительных услуг.")


class Element:
    def __init__(self, tag, attrs=()):
        self.tag, self.attrs, self.children = tag, dict(attrs), []

    def walk(self):
        yield self
        for child in self.children:
            if isinstance(child, Element):
                yield from child.walk()

    def text(self):
        return " ".join(" ".join(c.text() if isinstance(c, Element) else c
                                 for c in self.children).split())

    def has_class(self, name):
        return name in self.attrs.get("class", "").split()


class Page(HTMLParser):
    """Read public card markup; never run downloaded JavaScript."""
    VOID = {"area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"}

    def __init__(self, text):
        super().__init__(convert_charrefs=True)
        self.root = Element("document")
        self.stack = [self.root]
        self.feed(text)

    def handle_starttag(self, tag, attrs):
        node = Element(tag, attrs)
        self.stack[-1].children.append(node)
        if tag not in self.VOID:
            self.stack.append(node)

    def handle_endtag(self, tag):
        for i in range(len(self.stack)-1, 0, -1):
            if self.stack[i].tag == tag:
                del self.stack[i:]
                break

    def handle_data(self, text):
        self.stack[-1].children.append(text)


def iphoster(text):
    page = Page(text).root
    row = one(n for n in page.walk() if n.attrs.get("id") == "plansRow")
    if row.attrs.get("data-cur") != "USD":
        raise ValueError("Изменилась валюта IPHoster")
    card = one(n for n in row.walk() if n.tag == "article" and n.has_class("vps-plan")
               and any(x.has_class("badge") and x.text() == "VPS-NVE-S" for x in n.walk()))
    specs = [n.text() for n in card.walk() if n.tag == "li"]
    def spec(pattern):
        return int(one(m[1] for s in specs if (m := re.fullmatch(pattern, s))))
    if "IPv4 + IPv6 included" not in specs:
        raise ValueError("Изменились условия IPv4")
    return dict(plan="VPS-NVE-S", price_minor=cents(card.attrs["data-m"]), currency="USD",
                cpu=spec(r"(\d+) vCPU cores"), ram_mb=spec(r"(\d+) GB RAM")*1024,
                disk_gb=spec(r"(\d+) GB NVMe disk"), port_mbps=spec(r"(\d+) Mbps network"),
                price_note="Текущая цена · IPv4 включён", conditions="Помесячная цена страницы, без годовой предоплаты. IPv4 и IPv6 включены. Общий порт, безлимитный трафик с fair use; налоги и цену продления уточните при заказе.")


def vultr(text):
    data = json.loads(text)
    if data.get("meta", {}).get("links", {}).get("next"):
        raise ValueError("Каталог Vultr получен не полностью")
    p = one(x for x in data["plans"] if x["id"] == "vc2-1c-1gb")
    if p["type"] != "vc2" or p["disk_count"] != 1 or "ams" not in p["locations"] or not p["deploy_ondemand"]:
        raise ValueError("Изменились тариф или локация Vultr")
    price = p.get("location_cost", {}).get("ams", {}).get("monthly_cost", p["monthly_cost"])
    quota = p["bandwidth"]
    if not isinstance(quota, int) or quota <= 0:
        raise ValueError("Не указан включённый трафик Vultr")
    return dict(plan=p["id"], price_minor=cents(price), currency="USD", cpu=p["vcpu_count"],
                ram_mb=p["ram"], disk_gb=p["disk"], port_mbps=None,
                price_note="Без налогов · лимит трафика",
                conditions=f"Обычный Cloud Compute, Амстердам, без spot/preemptible. Месячная цена API в USD; включено {quota} GB трафика, превышение оплачивается отдельно. Скорость порта в API не указана. Налоги и дополнительные услуги уточните при заказе.")


def hetzner(text, page, ipv4):
    def monthly(raw, product, dc):
        data = json.loads(raw)
        if data["productId"] != product:
            raise ValueError("Изменился продукт Hetzner")
        location = one(x for x in data["locations"] if x["datacenter"] == dc and x["active"])
        return Decimal(location["prices"]["monthly"]["EUR"])
    price = monthly(text, "CLOUD_140", "NBG1") + monthly(ipv4, "CLOUD_21", "ALL")
    card = one(n for n in Page(page).root.walk() if n.has_class("cloud-matrix-product")
               and any(x.has_class("name-cell") and x.text() == "CPX02" for x in n.walk()))
    if card.has_class("not-available") or not any(n.attrs.get("product-key") == "CLOUD_140+CLOUD_21" for n in card.walk()):
        raise ValueError("Изменился тариф или доступность Hetzner")
    def cell(name):
        return one(n.text() for n in card.walk() if n.has_class(name))
    cpu = re.fullmatch(r"(\d+) AMD", cell("cpu-cell"))
    ram = re.fullmatch(r"(\d+) GB", cell("ram-cell"))
    disk = re.fullmatch(r"(\d+) GB NVMe", cell("drive-cell"))
    traffic = re.search(r"Traffic inclusive (\d+) TB", card.text())
    if not all((cpu, ram, disk, traffic)) or "NBG1" not in card.text():
        raise ValueError("Изменились характеристики Hetzner")
    return dict(plan="CPX02", price_minor=cents(price), currency="EUR", cpu=int(cpu[1]),
                ram_mb=int(ram[1])*1024, disk_gb=int(disk[1]), port_mbps=None,
                price_note="Без НДС · с платным IPv4",
                conditions=f"NBG1, месячный предел стоимости сервера плюс один Primary IPv4. Без НДС. Включено {traffic[1]} TB трафика, превышение оплачивается отдельно. Скорость общего порта не гарантирована. Наличие ресурсов проверьте при заказе.")


ADAPTERS = {"ovh": ovh, "itldc": itldc, "timeweb": timeweb, "firstvds": firstvds,
            "iphoster": iphoster, "vultr": vultr, "hetzner": hetzner}


def fetch(url):
    req = Request(url, headers={"User-Agent": "SecretTunnel-Catalog/1.0 (+https://github.com/openm1ke/Secret-Tunnel)"})
    with urlopen(req, timeout=30) as r:
        body = r.read(16_777_217)
    if len(body) > 16_777_216:
        raise ValueError("Источник больше 16 MiB")
    return body.decode("utf-8")


def timestamp(value):
    if not isinstance(value, str) or not re.fullmatch(r"20\d\d-\d\d-\d\dT\d\d:\d\d:\d\d(?:Z|\+00:00)", value):
        raise ValueError("Некорректная дата каталога")
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def validate(catalog):
    """Publication guard; CI also validates with the clients' Rust parser."""
    timestamp(catalog["generated_at"])
    offers = catalog["offers"]
    if catalog["schema_version"] != 1 or not 1 <= len(offers) <= 32:
        raise ValueError("Неподдерживаемая версия или размер каталога")
    ids = set()
    for o in offers:
        if not re.fullmatch(r"[A-Za-z0-9-]{1,64}", o["id"]) or o["id"] in ids:
            raise ValueError("Повторный или некорректный ID карточки")
        ids.add(o["id"])
        timestamp(o["verified_at"])
        if timestamp(o["verified_at"]) > timestamp(catalog["generated_at"]):
            raise ValueError("Дата проверки новее каталога")
        for key in ("provider_id", "provider", "company_country", "city", "plan", "conditions", "test_note"):
            if not isinstance(o[key], str) or not 1 <= len(o[key].encode()) <= 2000:
                raise ValueError(f"Некорректное поле {key}")
        if not re.fullmatch(r"[A-Z]{2}", o["country"]) or o["currency"] not in ("RUB", "EUR", "USD"):
            raise ValueError("Некорректная страна или валюта")
        for key in ("price_minor", "cpu", "ram_mb", "disk_gb"):
            if type(o[key]) is not int or not 0 < o[key] <= (100_000_000 if key == "price_minor" else 2**32-1):
                raise ValueError(f"Некорректное число {key}")
        port = o["port_mbps"]
        if port is not None and (type(port) is not int or not 0 < port <= 100_000):
            raise ValueError("Некорректная скорость порта")
        if o.get("price_note") is not None and (not isinstance(o["price_note"], str) or len(o["price_note"].encode()) > 240):
            raise ValueError("Некорректное пояснение цены")
        if o.get("update_error") is not None and (not isinstance(o["update_error"], str) or len(o["update_error"].encode()) > 2000):
            raise ValueError("Некорректная ошибка обновления")
        for key in ("source_url", "order_url", "test_source_url"):
            u = urlsplit(o[key])
            if u.scheme != "https" or not u.hostname or u.username or u.password or len(o[key]) > 2048:
                raise ValueError(f"Некорректный HTTPS URL {key}")
        host = o.get("test_host")
        port_number = o.get("test_port")
        if host is None and port_number is None:
            continue
        if not isinstance(host, str) or type(port_number) is not int:
            raise ValueError("Неполный тестовый адрес")
        try:
            ip = ipaddress.ip_address(host)
        except ValueError:
            if len(host) > 253 or "." not in host or any(not re.fullmatch(r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?", p) for p in host.split(".")):
                raise ValueError("Некорректный тестовый хост")
        else:
            if not ip.is_global or ip.is_multicast or any(ip in ipaddress.ip_network(n) for n in
                (["198.18.0.0/15", "192.0.0.0/24", "192.88.99.0/24"] if ip.version == 4 else ["::/3", "4000::/2", "8000::/1", "2001::/23", "2001:db8::/32", "2002::/16", "3fff::/20"])):
                raise ValueError("Непубличный тестовый IP")
        if o["test_port"] not in (80, 443):
            raise ValueError("Некорректный тестовый порт")
    return catalog


def load(path):
    with path.open("rb") as f:
        raw = f.read(MAX_CATALOG_BYTES+1)
    if len(raw) > MAX_CATALOG_BYTES:
        raise ValueError("Каталог больше 256 KiB")
    return validate(json.loads(raw))


def baseline(seed, published):
    """Code owns membership/targets; published data owns newer price facts."""
    validate(seed)
    if published is None:
        return copy.deepcopy(seed)
    validate(published)
    result = copy.deepcopy(seed)
    old = {o["id"]: o for o in published["offers"]}
    for o in result["offers"]:
        previous = old.get(o["id"])
        if previous and previous["provider_id"] == o["provider_id"] and timestamp(previous["verified_at"]) >= timestamp(o["verified_at"]):
            for field in FACTS:
                if field in previous:
                    o[field] = previous[field]
    result["generated_at"] = max((seed["generated_at"], published["generated_at"]), key=timestamp)
    return validate(result)


def update(catalog, read, now):
    validate(catalog)
    if timestamp(now) < timestamp(catalog["generated_at"]):
        raise ValueError("Нельзя обновить каталог задним числом")
    result = copy.deepcopy(catalog)
    failed = []
    for offer in result["offers"]:
        provider = offer["provider_id"]
        try:
            extras = {key: read(f"{provider}-{key}", url) for key, url in EXTRA_SOURCES.get(provider, {}).items()}
            facts = ADAPTERS[provider](read(provider, SOURCES[provider]), **extras)
            candidate = dict(offer, **facts, verified_at=now, source_url=SOURCES[provider], update_error=None)
            validate(dict(catalog, generated_at=now, offers=[candidate]))
            offer.update(candidate)
        except (ValueError, KeyError, IndexError, TypeError, ArithmeticError, OSError, RecursionError) as e:
            offer["update_error"] = "Источник изменился или недоступен; показаны прежние данные."
            failed.append(f"{provider}: {e}")
    result["generated_at"] = now
    return validate(result), failed


def write_json(path, value):
    raw = json.dumps(value, ensure_ascii=False, indent=2)+"\n"
    if "schema_version" in value and len(raw.encode("utf-8")) > MAX_CATALOG_BYTES:
        raise ValueError("Результат больше 256 KiB")
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix+".tmp")
    tmp.write_text(raw, encoding="utf-8")
    tmp.replace(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--catalog", type=Path, default=ROOT/"site/catalog/hosting-v1.json")
    parser.add_argument("--source-dir", type=Path, help="Offline source snapshots: <provider>.source")
    parser.add_argument("--output", type=Path, help="Write candidate instead of replacing the catalog")
    parser.add_argument("--published", type=Path, help="Last published catalog; missing file is allowed only for first publication")
    parser.add_argument("--report", type=Path, help="Write machine-readable source health report")
    parser.add_argument("--summary", type=Path, help="Append a GitHub job summary")
    parser.add_argument("--allow-partial", action="store_true", help="Exit 0 for validated partial updates; total failure remains exit 1")
    args = parser.parse_args()
    cache = {}
    def source(provider, url):
        if url not in cache:
            cache[url] = (args.source_dir/f"{provider}.source").read_text() if args.source_dir else fetch(url)
        return cache[url]
    initial = baseline(load(args.catalog), load(args.published) if args.published and args.published.exists() else None)
    now = datetime.now(timezone.utc).isoformat(timespec="seconds")
    value, errors = update(initial, source, now)
    raw = json.dumps(value, ensure_ascii=False).encode()
    if len(raw) > MAX_CATALOG_BYTES:
        raise ValueError("Результат больше 256 KiB")
    write_json(args.output or args.catalog, value)
    successful = sum(not o.get("update_error") for o in value["offers"])
    report = dict(generated_at=now, status="ok" if not errors else "partial" if successful else "failed",
                  successful=successful, total=len(value["offers"]), failures=errors)
    if args.report:
        write_json(args.report, report)
    if args.summary:
        with args.summary.open("a", encoding="utf-8") as f:
            f.write(f"## Hosting catalog\n\nSources refreshed: **{successful}/{len(value['offers'])}**. Status: **{report['status']}**.\n\n")
            f.write("| Provider | Result | Verified (UTC) |\n| --- | --- | --- |\n")
            for o in value["offers"]:
                f.write(f"| {o['provider_id']} | {'Previous data retained' if o.get('update_error') else 'Updated'} | {o['verified_at']} |\n")
    for error in errors:
        print(error, file=sys.stderr)
    return int(bool(errors) and (not args.allow_partial or not successful))


if __name__ == "__main__":
    sys.exit(main())
