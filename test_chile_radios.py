#!/usr/bin/env python3
"""
Concurrent radio stream tester for Chilean stations
Reads stations from chile_stations.toml and tests them concurrently
"""

import subprocess
import tomllib
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
import time

MPV_PATH = Path(__file__).parent / "external" / "mpv"
if not MPV_PATH.exists():
    MPV_PATH = "mpv"  # fallback to system mpv


def test_station(station):
    """Test a single radio station stream"""
    name = station.get("name", "Unknown")
    url = station.get("stream_url", "")

    if not url:
        return {"name": name, "status": "skipped", "error": "No URL"}

    # Use mpv with null audio output, timeout after 5 seconds
    cmd = [
        str(MPV_PATH),
        "--no-video",
        "--ao=null",  # Silent mode - no audio output
        "--msg-level=all=error",  # Only show errors
        "--really-quiet",  # Minimal output
        "--length=5",  # Try to play for 5 seconds max
        url,
    ]

    try:
        result = subprocess.run(cmd, timeout=10, capture_output=True, text=True)

        # Check if mpv succeeded (exit code 0) or was terminated by timeout (which is ok)
        if (
            result.returncode == 0
            or result.returncode == -15
            or result.returncode == 255
        ):
            return {"name": name, "status": "working", "url": url}
        else:
            # Check stderr for clues
            stderr = result.stderr.lower()
            if "error" in stderr and (
                "404" in stderr or "403" in stderr or "failed" in stderr
            ):
                return {
                    "name": name,
                    "status": "failed",
                    "error": f"Exit code: {result.returncode}",
                }
            else:
                # If no clear error message, it might actually be working
                return {
                    "name": name,
                    "status": "working",
                    "url": url,
                    "note": f"exit_code={result.returncode}",
                }

    except subprocess.TimeoutExpired:
        # Timeout means it was playing for 5+ seconds - that's good!
        return {
            "name": name,
            "status": "working",
            "url": url,
            "note": "timeout (playing)",
        }
    except Exception as e:
        return {"name": name, "status": "error", "error": str(e)}


def main():
    # Read TOML file
    toml_path = Path(__file__).parent / "chile_stations.toml"

    try:
        with open(toml_path, "rb") as f:
            data = tomllib.load(f)
    except Exception as e:
        print(f"Error reading TOML file: {e}")
        sys.exit(1)

    stations = data.get("stations", [])

    # Filter out stations without URLs or marked as unavailable
    test_stations = [
        s for s in stations if s.get("stream_url") and s.get("status") != "unavailable"
    ]

    print(f"Testing {len(test_stations)} Chilean radio stations...")
    print(f"Using mpv: {MPV_PATH}")
    print(f"Workers: 5 (concurrent)")
    print("=" * 70)
    print()

    working = []
    failed = []
    skipped = []

    start_time = time.time()

    # Test concurrently with 5 workers
    with ThreadPoolExecutor(max_workers=5) as executor:
        futures = {
            executor.submit(test_station, station): station for station in test_stations
        }

        for future in as_completed(futures):
            result = future.result()
            name = result["name"]
            status = result["status"]

            if status == "working":
                note = result.get("note", "")
                url = (
                    result.get("url", "")[:50] + "..."
                    if len(result.get("url", "")) > 50
                    else result.get("url", "")
                )
                print(f"✓ {name}")
                if note:
                    print(f"  Note: {note}")
                working.append(result)
            elif status == "failed":
                error = result.get("error", "")
                print(f"✗ {name} - {error}")
                failed.append(result)
            elif status == "skipped":
                error = result.get("error", "")
                print(f"⊘ {name} - {error}")
                skipped.append(result)
            else:
                error = result.get("error", "")
                print(f"? {name} - {error}")
                failed.append(result)

    elapsed = time.time() - start_time

    print()
    print("=" * 70)
    print(
        f"Results: {len(working)} working, {len(failed)} failed, {len(skipped)} skipped"
    )
    print(f"Time: {elapsed:.1f}s")
    print()

    # Summary
    if working:
        print("Working Stations:")
        for w in working:
            print(f"  ✓ {w['name']}")

    if failed:
        print()
        print("Failed Stations:")
        for f in failed:
            print(f"  ✗ {f['name']}")


if __name__ == "__main__":
    main()
