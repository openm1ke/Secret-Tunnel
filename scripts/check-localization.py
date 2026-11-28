#!/usr/bin/env python3
"""Validate the shared catalog and explicit translation calls without SDK dependencies."""
import collections
import json
import pathlib
import re
import xml.etree.ElementTree as ET

ROOT = pathlib.Path(__file__).resolve().parents[1]
errors = []


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            errors.append(f"Duplicate message: {key!r}")
        result[key] = value
    return result


catalog = json.loads((ROOT / "locales/en.json").read_text(encoding="utf-8"), object_pairs_hook=unique_object)
placeholders = lambda value: collections.Counter(re.findall(r"\{\d+\}", value))
formats = lambda value: collections.Counter(re.findall(r"%(?:\d+\$)?[-+0-9.]*[sdif@]", value))
for source, english in catalog.items():
    if not isinstance(english, str) or not english.strip():
        errors.append(f"Empty translation: {source!r}")
    elif re.search("[А-Яа-яЁё]", english):
        errors.append(f"Russian text in English translation: {source!r}")
    elif placeholders(source) != placeholders(english) or formats(source) != formats(english):
        errors.append(f"Changed arguments: {source!r}")

calls = 0
for directory, extension in [("src", "tsx"), ("android/app/src/main/java/dev/secrettunnel/android", "kt"), ("ios/App", "swift")]:
    for file in (ROOT / directory).glob(f"*.{extension}"):
        for match in re.finditer(r'\b(?:t|tr)\(\s*("(?:\\.|[^"\\])*")', file.read_text(encoding="utf-8")):
            source = json.loads(match[1].replace(r"\$", "$"))
            if re.search("[А-Яа-яЁё]", source):
                calls += 1
                if source not in catalog:
                    errors.append(f"{file.relative_to(ROOT)}: missing {source!r}")

for offer in json.loads((ROOT / "site/catalog/hosting-v1.json").read_text(encoding="utf-8"))["offers"]:
    for field in ["city", "company_country", "conditions", "test_note", "price_note"]:
        value = offer.get(field)
        if value and re.search("[А-Яа-яЁё]", value) and value not in catalog:
            errors.append(f"Catalog {offer['id']}.{field}: missing translation")

resources = ROOT / "android/app/src/main/res"
native = {item.attrib["name"]: item for item in ET.parse(resources / "values/strings.xml").getroot()}
native_english = {item.attrib["name"]: item for item in ET.parse(resources / "values-en/strings.xml").getroot()}
for name, item in native.items():
    if item.attrib.get("translatable") == "false":
        continue
    if name not in native_english:
        errors.append(f"Android resource {name}: missing English translation")
    elif formats("".join(item.itertext())) != formats("".join(native_english[name].itertext())):
        errors.append(f"Android resource {name}: changed format arguments")
for name in native_english:
    if name not in native:
        errors.append(f"Android resource {name}: missing default resource")

if errors:
    raise SystemExit("\n".join(errors))
print(f"Localization: {len(catalog)} messages, {calls} call sites; arguments and catalog copy verified")
