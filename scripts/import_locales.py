#!/usr/bin/env python3
"""Import the original app's i18next catalogs and flatten them to one JSON per locale.

Source: github.com/777genius/agent-teams-ai
  src/features/localization/renderer/locales/<code>/<namespace>.json
Output: crates/rteams-services/src/locales/<code>.json, a flat sorted map
  {"team.editor.actions.save": "Save", ...}  (namespace = file name prefix).
Requires the `gh` CLI (authenticated). Usage: scripts/import_locales.py [code ...]
"""
import base64, json, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

REPO = "777genius/agent-teams-ai"
BASE = "src/features/localization/renderer/locales"
OUT = Path(__file__).resolve().parent.parent / "crates/rteams-services/src/locales"


def gh(*args):
    return subprocess.run(["gh", "api", *args], check=True, capture_output=True, text=True).stdout


def flatten(prefix, node, out):
    if isinstance(node, dict):
        for k, v in node.items():
            flatten(f"{prefix}.{k}" if prefix else k, v, out)
    elif isinstance(node, str):
        out[prefix] = node
    elif node is not None:
        out[prefix] = str(node)


def fetch(path):
    for attempt in range(3):
        try:
            raw = gh(f"repos/{REPO}/contents/{path}", "--jq", ".content")
            return json.loads(base64.b64decode(raw))
        except Exception:
            if attempt == 2:
                raise


def main():
    tree = gh(f"repos/{REPO}/git/trees/main?recursive=1", "--jq",
              f'.tree[]|select(.path|startswith("{BASE}/"))|select(.path|endswith(".json"))|.path')
    by_locale = {}
    for p in tree.split():
        code, fname = p[len(BASE) + 1:].split("/")
        by_locale.setdefault(code, []).append((fname[:-5], p))
    wanted = sys.argv[1:] or sorted(by_locale)
    OUT.mkdir(parents=True, exist_ok=True)

    def do(code):
        flat = {}
        for ns, path in sorted(by_locale[code]):
            flatten(ns, fetch(path), flat)
        text = json.dumps(dict(sorted(flat.items())), ensure_ascii=False, separators=(",", ":"))
        (OUT / f"{code}.json").write_text(text + "\n", encoding="utf-8")
        return code, len(flat)

    with ThreadPoolExecutor(8) as ex:
        for code, n in ex.map(do, wanted):
            print(f"{code}: {n} keys")


if __name__ == "__main__":
    main()
