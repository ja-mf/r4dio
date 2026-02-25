#!/usr/bin/env python3
"""
Tiny HTTP server exposing PipeWire/PulseAudio volume via pactl.

Endpoints (all accept GET or POST):
  GET  /api/volume        -> {"volume": 54, "mute": false}
  POST /api/volume/<0-100>  -> set volume %
  POST /api/mute/1          -> mute
  POST /api/mute/0          -> unmute
  GET  /api/mute            -> {"mute": false}

Designed to be consumed by homebridge-http-lightbulb:
  on/off  = unmute/mute
  brightness = volume %
"""

import http.server
import subprocess
import json
import re
import sys
import os
import logging

PORT = int(os.environ.get("PACTL_HTTP_PORT", "8990"))
SINK = os.environ.get("PACTL_SINK", "@DEFAULT_SINK@")

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(message)s",
    stream=sys.stdout,
)
log = logging.getLogger("pactl-http")


def run(cmd):
    result = subprocess.run(
        cmd, capture_output=True, text=True, timeout=5
    )
    if result.returncode != 0:
        raise RuntimeError(f"Command failed: {' '.join(cmd)}\n{result.stderr.strip()}")
    return result.stdout.strip()


def get_volume():
    """Return integer 0-100."""
    out = run(["pactl", "get-sink-volume", SINK])
    # "Volume: aux0: 32768 /  50% / ..."
    m = re.search(r"/\s*(\d+)%", out)
    if not m:
        raise RuntimeError(f"Could not parse volume: {out!r}")
    return int(m.group(1))


def get_mute():
    """Return bool."""
    out = run(["pactl", "get-sink-mute", SINK])
    return "yes" in out.lower()


def set_volume(pct):
    pct = max(0, min(100, int(pct)))
    run(["pactl", "set-sink-volume", SINK, f"{pct}%"])
    return pct


def set_mute(muted: bool):
    run(["pactl", "set-sink-mute", SINK, "1" if muted else "0"])


class Handler(http.server.BaseHTTPRequestHandler):

    def log_message(self, fmt, *args):
        log.info(fmt % args)

    def send_json(self, code, obj):
        body = json.dumps(obj, separators=(",", ":")).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def send_err(self, msg):
        self.send_json(500, {"error": msg})

    def handle_request(self):
        path = self.path.rstrip("/")
        try:
            # GET /api/volume
            if path == "/api/volume":
                self.send_json(200, {"volume": get_volume(), "mute": get_mute()})

            # GET /api/mute
            elif path == "/api/mute":
                self.send_json(200, {"mute": get_mute()})

            # POST /api/volume/<pct>
            elif path.startswith("/api/volume/"):
                pct = int(path.split("/")[-1])
                v = set_volume(pct)
                self.send_json(200, {"volume": v})

            # POST /api/mute/1 or /api/mute/0
            elif path.startswith("/api/mute/"):
                val = path.split("/")[-1]
                muted = val in ("1", "true", "yes")
                set_mute(muted)
                self.send_json(200, {"mute": muted})

            else:
                self.send_json(404, {"error": "not found"})

        except Exception as e:
            log.error("Error handling %s: %s", path, e)
            self.send_err(str(e))

    def do_GET(self):
        self.handle_request()

    def do_POST(self):
        self.handle_request()


if __name__ == "__main__":
    server = http.server.HTTPServer(("127.0.0.1", PORT), Handler)
    log.info("pactl-http listening on 127.0.0.1:%d (sink: %s)", PORT, SINK)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        log.info("Shutting down")
