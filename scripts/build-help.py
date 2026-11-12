#!/usr/bin/env python3
"""Builds the in-app help from the website: docs/help/*.md.

The site pages stay the single source. The apps fetch these Markdown files
from the repository (and ship a copy for offline use), so editing
site/guide.html or site/knowledge.html and running this script updates the
help everywhere. `--check` fails when the files are out of date (CI).

  python3 scripts/build-help.py [--check]
"""
import html
import re
import sys
from html.parser import HTMLParser
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SITE = "https://openm1ke.github.io/Secret-Tunnel/"
OUT = ROOT / "docs" / "help"
VOID = {"img", "br", "hr", "input", "meta", "link", "source"}


class Node:
    def __init__(self, tag, attrs, parent=None):
        self.tag, self.attrs, self.parent, self.children = tag, dict(attrs), parent, []

    def cls(self):
        return self.attrs.get("class", "").split()

    def text(self):
        return "".join(c if isinstance(c, str) else c.text() for c in self.children)


class Tree(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.root = Node("root", {})
        self.current = self.root

    def handle_starttag(self, tag, attrs):
        node = Node(tag, attrs, self.current)
        self.current.children.append(node)
        if tag not in VOID:
            self.current = node

    def handle_startendtag(self, tag, attrs):
        self.current.children.append(Node(tag, attrs, self.current))

    def handle_endtag(self, tag):
        node = self.current
        while node is not self.root and node.tag != tag:
            node = node.parent
        if node is not self.root:
            self.current = node.parent

    def handle_data(self, data):
        self.current.children.append(data)


def find_all(node, test):
    for child in node.children:
        if isinstance(child, Node):
            if test(child):
                yield child
            yield from find_all(child, test)


def absolute(url):
    if url.startswith(("http://", "https://", "mailto:", "#")):
        return url
    return SITE + url.lstrip("./")


class Renderer:
    def __init__(self, os_name=None):
        self.os = os_name

    def hidden(self, node):
        if not isinstance(node, Node):
            return False
        scope = node.attrs.get("data-os")
        if scope and self.os and self.os not in scope.split():
            return True
        classes = node.cls()
        # site-only: text about the page's own controls; app-only shows only here.
        return any(c in classes for c in ("nonroot", "illo", "copy", "site-only")) or node.tag in ("script", "style", "svg")

    def plain(self, node):
        """Text of a terminal picture, with the key path of this system."""
        out = []
        for child in node.children:
            if isinstance(child, str):
                out.append(child)
            elif "keypath" in child.cls():
                out.append({"win": "C:\\Users\\user\\.ssh\\secret-tunnel", "linux": "/home/user/.ssh/secret-tunnel"}.get(self.os, "/Users/user/.ssh/secret-tunnel"))
            elif not self.hidden(child):
                out.append(self.plain(child))
        return "".join(out)

    def inline(self, node):
        out = []
        for child in node.children:
            if isinstance(child, str):
                out.append(re.sub(r"\s+", " ", child))
                continue
            if self.hidden(child):
                continue
            tag, inner = child.tag, None
            if tag in ("b", "strong"):
                inner = self.inline(child).strip()
                out.append(f"**{inner}**" if inner else "")
            elif tag in ("i", "em"):
                inner = self.inline(child).strip()
                out.append(f"*{inner}*" if inner else "")
            elif tag in ("code", "kbd"):
                out.append("`" + child.text().strip() + "`")
            elif tag == "a":
                inner = self.inline(child).strip()
                out.append(f"[{inner}]({absolute(child.attrs.get('href', ''))})")
            elif tag == "br":
                out.append("  \n")
            elif tag == "span" and "keypath" in child.cls():
                out.append({"win": "C:\\Users\\user\\.ssh\\secret-tunnel", "linux": "/home/user/.ssh/secret-tunnel"}.get(self.os, "/Users/user/.ssh/secret-tunnel"))
            elif tag == "span" and "path" in child.cls():
                parts = [self.inline(part).strip() for part in child.children if isinstance(part, Node)]
                out.append(" → ".join(f"«{p}»" for p in parts))
            else:
                out.append(self.inline(child))
        return "".join(out)

    def block(self, node, depth=0):
        """Markdown for the block-level children of `node`."""
        parts = []
        loose = []

        def flush():
            text = "".join(loose).strip()
            if text:
                parts.append(text)
            loose.clear()

        for child in node.children:
            if isinstance(child, str):
                loose.append(re.sub(r"\s+", " ", child))
                continue
            if self.hidden(child):
                continue
            tag, classes = child.tag, child.cls()
            if tag in ("a", "b", "strong", "code", "kbd", "span", "em", "i", "br") and "cmdbox" not in classes:
                loose.append(self.inline(wrap(child)))
                continue
            flush()
            if tag in ("h1", "h2", "h3"):
                level = {"h1": "#", "h2": "##", "h3": "###"}[tag]
                anchor = child.attrs.get("id") or (node.attrs.get("id") if tag == "h2" else None)
                if anchor:
                    parts.append(f'<a id="{anchor}"></a>')
                parts.append(f"{level} {child.text().strip()}")
            elif tag == "p":
                text = self.inline(child).strip()
                if not text:
                    continue
                if "note" in classes:
                    text = "> " + text
                elif "part" in classes:
                    text = f"# {text}"
                parts.append(text)
            elif tag == "pre" or (tag == "div" and "cmdbox" in classes):
                pre = child if tag == "pre" else next(find_all(child, lambda n: n.tag == "pre"))
                code = pre.text().strip("\n")
                language = "powershell" if self.os == "win" and "$env:" in code else "bash" if "cmd" in pre.cls() else ""
                parts.append(f"```{language}\n{code}\n```")
            elif tag == "figure" and "term" in classes:
                pre = next(find_all(child, lambda n: n.tag == "pre"))
                parts.append("```text\n" + self.plain(pre).strip("\n") + "\n```")
            elif tag == "figure" and "shot" in classes:
                image = next(find_all(child, lambda n: n.tag == "img"))
                attrs = [f'class="shot"']
                if child.attrs.get("data-pins"):
                    attrs.append(f'data-pins="{child.attrs["data-pins"]}"')
                alt = html.escape(image.attrs.get("alt", ""), quote=True)
                parts.append(f'<figure {" ".join(attrs)}><img src="{absolute(image.attrs["src"])}" alt="{alt}"></figure>')
            elif tag in ("ul", "ol"):
                parts.append(self.listing(child, depth))
            elif tag == "table":
                parts.append(self.table(child))
            elif tag == "details":
                summary = next((n for n in child.children if isinstance(n, Node) and n.tag == "summary"), None)
                body = Node("div", {}, None)
                body.children = [n for n in child.children if n is not summary]
                if summary is not None:
                    parts.append(f"**{summary.text().strip()}**")
                parts.append(self.block(body, depth))
            elif tag in ("div", "section", "article", "main", "figure", "label", "aside", "header"):
                parts.append(self.block(child, depth))
            else:
                parts.append(self.inline(child).strip())
        flush()
        return "\n\n".join(p for p in parts if p.strip())

    def listing(self, node, depth):
        ordered = node.tag == "ol"
        lines = []
        number = 0
        for item in node.children:
            if not isinstance(item, Node) or item.tag != "li" or self.hidden(item):
                continue
            number += 1
            body = self.block(item, depth + 1)
            marker = f"{number}." if ordered else "-"
            indent = " " * (len(marker) + 1)
            first, *rest = body.split("\n") if body else [""]
            lines.append(f"{marker} {first}")
            lines.extend(indent + line if line.strip() else "" for line in rest)
        return "\n".join(lines)

    def table(self, node):
        rows = []
        for row in find_all(node, lambda n: n.tag == "tr"):
            if self.hidden(row):
                continue
            cells = [self.inline(cell).strip().replace("|", "\\|") for cell in row.children
                     if isinstance(cell, Node) and cell.tag in ("th", "td") and not self.hidden(cell)]
            if cells:
                rows.append(cells)
        if not rows:
            return ""
        width = max(len(r) for r in rows)
        rows = [r + [""] * (width - len(r)) for r in rows]
        head = rows[0] if any(c for c in rows[0]) else [""] * width
        out = ["| " + " | ".join(head) + " |", "|" + "---|" * width]
        out += ["| " + " | ".join(r) + " |" for r in rows[1:]]
        return "\n".join(out)


def wrap(node):
    holder = Node("span", {}, None)
    holder.children = [node]
    return holder


def page(path):
    tree = Tree()
    tree.feed((ROOT / "site" / path).read_text(encoding="utf-8"))
    return next(find_all(tree.root, lambda n: n.tag == "main"))


def tidy(text):
    text = re.sub(r"\n{3,}", "\n\n", text)
    return text.strip() + "\n"


def build():
    files = {}
    guide = page("guide.html")
    for os_name, title in (("mac", "macOS"), ("win", "Windows")):
        intro = (f"<!-- Generated by scripts/build-help.py from site/guide.html for {title}. Edit the site page, not this file. -->\n\n")
        files[f"setup-{'macos' if os_name == 'mac' else 'windows'}.md"] = tidy(intro + Renderer(os_name).block(guide))
    knowledge = page("knowledge.html")
    for empty in find_all(knowledge, lambda n: "empty-search" in n.cls()):
        empty.children = []
    files["knowledge.md"] = tidy("<!-- Generated by scripts/build-help.py from site/knowledge.html. Edit the site page, not this file. -->\n\n"
                                 + Renderer().block(knowledge))
    return files


def main():
    check = "--check" in sys.argv
    stale = []
    OUT.mkdir(parents=True, exist_ok=True)
    for name, text in build().items():
        target = OUT / name
        if check:
            if not target.exists() or target.read_text(encoding="utf-8") != text:
                stale.append(name)
        else:
            target.write_text(text, encoding="utf-8")
            print(f"wrote docs/help/{name} ({len(text)} chars)")
    if stale:
        sys.exit("docs/help is out of date, run scripts/build-help.py: " + ", ".join(stale))


if __name__ == "__main__":
    main()
