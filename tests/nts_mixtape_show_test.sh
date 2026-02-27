#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <nts-infinite-mixtape-url>" >&2
  echo "Example: $0 https://www.nts.live/infinite-mixtapes/slow-focus" >&2
  exit 1
fi

URL="$1"

python3 - "$URL" <<'PY'
import json
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request


def fetch_text(url: str, timeout: int = 30) -> str:
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.read().decode("utf-8", "replace")


def parse_alias(mixtape_url: str) -> str:
    parsed = urllib.parse.urlparse(mixtape_url)
    path = parsed.path.rstrip("/")
    m = re.search(r"/infinite-mixtapes/([^/?#]+)$", path)
    if not m:
        raise ValueError("URL must look like https://www.nts.live/infinite-mixtapes/<alias>")
    return m.group(1)


def extract_bundle_url(html: str) -> str:
    srcs = re.findall(r'<script[^>]+src="([^"]+)"', html, re.I)
    for src in srcs:
        if "/js/app.min." in src and src.endswith(".js"):
            if src.startswith("http"):
                return src
            return "https://www.nts.live" + src
    raise RuntimeError("Could not find NTS app bundle URL in page HTML")


def extract_prod_firebase(js: str) -> tuple[str, str]:
    m = re.search(r'apiKey:"(AIza[^"]+)"[^{}]*?projectId:"(nts-ios-app)"', js)
    if not m:
        raise RuntimeError("Could not extract Firebase prod apiKey/projectId from bundle")
    return m.group(1), m.group(2)


def field_string(fields: dict, name: str):
    v = fields.get(name, {})
    return v.get("stringValue") or v.get("timestampValue") or v.get("integerValue")


def run_query(project: str, api_key: str, alias: str):
    endpoint = (
        f"https://firestore.googleapis.com/v1/projects/{project}"
        f"/databases/(default)/documents:runQuery?key={api_key}"
    )
    payload = {
        "structuredQuery": {
            "from": [{"collectionId": "mixtape_titles"}],
            "where": {
                "fieldFilter": {
                    "field": {"fieldPath": "mixtape_alias"},
                    "op": "EQUAL",
                    "value": {"stringValue": alias},
                }
            },
            "orderBy": [{"field": {"fieldPath": "started_at"}, "direction": "DESCENDING"}],
            "limit": 1,
        }
    }

    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        endpoint,
        data=data,
        headers={"Content-Type": "application/json", "User-Agent": "Mozilla/5.0"},
    )
    with urllib.request.urlopen(req, timeout=30) as r:
        rows = json.loads(r.read().decode("utf-8", "replace"))

    if not isinstance(rows, list) or not rows:
        return None
    doc = rows[0].get("document")
    if not doc:
        return None

    fields = doc.get("fields", {})
    title = field_string(fields, "title")
    show_alias = field_string(fields, "show_alias")
    episode_alias = field_string(fields, "episode_alias")

    episode_url = None
    if show_alias:
        episode_url = f"https://www.nts.live/shows/{show_alias}"
        if episode_alias:
            episode_url += f"/episodes/{episode_alias}"

    return {
        "title": title,
        "episode_url": episode_url,
    }


def main(mixtape_url: str) -> int:
    t0 = time.perf_counter()
    try:
        alias = parse_alias(mixtape_url)
        html = fetch_text(mixtape_url)
        bundle_url = extract_bundle_url(html)
        js = fetch_text(bundle_url)
        api_key, project = extract_prod_firebase(js)
        row = run_query(project, api_key, alias)

        title = row.get("title") if row else None
        episode_url = row.get("episode_url") if row else None

        elapsed_ms = int((time.perf_counter() - t0) * 1000)

        if title:
            print(f"show: {title}")
            if episode_url:
                print(f"url: {episode_url}")
            else:
                print("url: (not announced)")
        else:
            print("not announced")

        print(f"elapsed_ms: {elapsed_ms}")
        return 0
    except urllib.error.HTTPError as e:
        elapsed_ms = int((time.perf_counter() - t0) * 1000)
        print(f"error: HTTP {e.code}")
        print(f"elapsed_ms: {elapsed_ms}")
        return 2
    except Exception as e:
        elapsed_ms = int((time.perf_counter() - t0) * 1000)
        print(f"error: {e}")
        print(f"elapsed_ms: {elapsed_ms}")
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1]))
PY
