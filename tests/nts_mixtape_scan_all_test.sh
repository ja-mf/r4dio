#!/usr/bin/env bash
set -euo pipefail

python3 - "$@" <<'PY'
import argparse
import json
import re
import sys
import time
import urllib.error
import urllib.request


def fetch_json(url: str, timeout: int = 30):
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0", "Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode("utf-8", "replace"))


def fetch_text(url: str, timeout: int = 30) -> str:
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.read().decode("utf-8", "replace")


def get_aliases() -> list[str]:
    payload = fetch_json("https://www.nts.live/api/v2/mixtapes")
    results = payload.get("results", []) if isinstance(payload, dict) else []
    aliases = []
    for item in results:
        if not isinstance(item, dict):
            continue
        alias = item.get("mixtape_alias")
        if alias:
            aliases.append(alias)
    aliases = sorted(set(aliases))
    if not aliases:
        raise RuntimeError("No mixtape aliases returned by /api/v2/mixtapes")
    return aliases


def extract_bundle_url(page_html: str) -> str:
    srcs = re.findall(r'<script[^>]+src="([^"]+)"', page_html, re.I)
    for src in srcs:
        if "/js/app.min." in src and src.endswith(".js"):
            if src.startswith("http"):
                return src
            return "https://www.nts.live" + src
    raise RuntimeError("Could not find NTS app bundle URL")


def extract_prod_firebase(js: str) -> tuple[str, str]:
    m = re.search(r'apiKey:"(AIza[^"]+)"[^{}]*?projectId:"(nts-ios-app)"', js)
    if not m:
        raise RuntimeError("Could not extract Firebase prod apiKey/projectId")
    return m.group(2), m.group(1)


def field_string(fields: dict, name: str):
    value = fields.get(name, {})
    return value.get("stringValue") or value.get("timestampValue") or value.get("integerValue")


def fetch_latest_episode(project: str, api_key: str, alias: str) -> dict:
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

    req = urllib.request.Request(
        endpoint,
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json", "User-Agent": "Mozilla/5.0"},
    )
    with urllib.request.urlopen(req, timeout=30) as r:
        rows = json.loads(r.read().decode("utf-8", "replace"))

    if not isinstance(rows, list) or not rows:
        return {"title": None, "url": None, "started_at": None}

    doc = rows[0].get("document")
    if not isinstance(doc, dict):
        return {"title": None, "url": None, "started_at": None}

    fields = doc.get("fields", {})
    title = field_string(fields, "title")
    show_alias = field_string(fields, "show_alias")
    episode_alias = field_string(fields, "episode_alias")
    started_at = field_string(fields, "started_at")

    url = None
    if show_alias:
        url = f"https://www.nts.live/shows/{show_alias}"
        if episode_alias:
            url += f"/episodes/{episode_alias}"

    return {"title": title, "url": url, "started_at": started_at}


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Probe all NTS infinite mixtapes for current announced episode (title/url)."
    )
    parser.add_argument(
        "--delay-seconds",
        type=float,
        default=0.40,
        help="Pause between aliases to reduce request burst (default: 0.40)",
    )
    parser.add_argument(
        "--bootstrap-url",
        default="https://www.nts.live/infinite-mixtapes/slow-focus",
        help="Page used once to discover current bundle URL (default: slow-focus)",
    )
    parser.add_argument(
        "--max-aliases",
        type=int,
        default=0,
        help="If >0, only process first N aliases (debug mode)",
    )
    args = parser.parse_args()

    run_start = time.perf_counter()

    aliases = get_aliases()
    if args.max_aliases > 0:
        aliases = aliases[: args.max_aliases]

    page_html = fetch_text(args.bootstrap_url)
    bundle_url = extract_bundle_url(page_html)
    bundle_js = fetch_text(bundle_url)
    project, api_key = extract_prod_firebase(bundle_js)

    total = len(aliases)
    estimated_seconds = total * (0.30 + max(args.delay_seconds, 0.0)) + 1.0
    print(f"aliases: {total}")
    print(f"strategy: 1 mixtapes list + 1 page + 1 bundle + {total} Firestore queries (sequential)")
    print(f"throttle: {args.delay_seconds:.2f}s between aliases")
    print(f"estimated_pass_seconds: ~{estimated_seconds:.1f}")
    print("--- live updates ---")
    sys.stdout.flush()

    with_url = 0
    title_only = 0
    not_announced = 0
    errors = 0

    for i, alias in enumerate(aliases, start=1):
        t0 = time.perf_counter()
        try:
            data = fetch_latest_episode(project, api_key, alias)
            title = data.get("title")
            url = data.get("url")

            if title and url:
                with_url += 1
                status = "title+url"
                detail = f"{title} | {url}"
            elif title:
                title_only += 1
                status = "title-only"
                detail = title
            else:
                not_announced += 1
                status = "not-announced"
                detail = "not announced"

            dt_ms = int((time.perf_counter() - t0) * 1000)
            print(f"[{i:02d}/{total:02d}] {alias:<24} {status:<13} {dt_ms:>4}ms  {detail}")
        except Exception as e:
            errors += 1
            dt_ms = int((time.perf_counter() - t0) * 1000)
            print(f"[{i:02d}/{total:02d}] {alias:<24} error         {dt_ms:>4}ms  {e}")

        sys.stdout.flush()
        if i < total and args.delay_seconds > 0:
            time.sleep(args.delay_seconds)

    elapsed = time.perf_counter() - run_start
    print("--- summary ---")
    print(f"with_url: {with_url}")
    print(f"title_only: {title_only}")
    print(f"not_announced: {not_announced}")
    print(f"errors: {errors}")
    print(f"elapsed_seconds: {elapsed:.2f}")
    if total > 0:
        print(f"avg_seconds_per_alias: {elapsed / total:.2f}")
    return 0 if errors == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
PY
