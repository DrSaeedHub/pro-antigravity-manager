"""A pretend Google for the demo: the OAuth token endpoint and the three Cloud Code calls the
quota service makes (loadCodeAssist, retrieveUserQuotaSummary, fetchAvailableModels), answering
from accounts.json in the shapes the app parses (crates/quota/src/parse.rs): the quota summary has
the "Gemini Models" group (its weekly limit from `gemini_weekly`, its 5-hour one the tightest
Gemini model) and the Claude and GPT group (`five_hour`, `weekly`). It also plays the
project's GitHub releases under /gh/owner/repo: latest.json when a session sets `release`, and a
4 MiB pretend package sent slowly, so the update sheet can be seen mid-download.

Each account's refresh token is `demo-refresh-<name>` and its access token `demo-access-<name>`.
An account marked `expired` refuses its refresh token with `invalid_grant`, as Google does for a
revoked sign-in.
"""

from __future__ import annotations

import hashlib
import json
import re
import threading
import time
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs

_DURATION = re.compile(r"(\d+)\s*([dhm])")


def _after(now: datetime, text: str | None) -> str | None:
    """`now` plus a duration such as `3d 4h` or `48m`, as RFC 3339."""
    if not text:
        return None
    minutes = 0
    for amount, unit in _DURATION.findall(text):
        minutes += int(amount) * {"d": 1440, "h": 60, "m": 1}[unit]
    return (now + timedelta(minutes=minutes)).strftime("%Y-%m-%dT%H:%M:%SZ")


class PretendGoogle:
    """The pretend service, on 127.0.0.1 and a free port."""

    def __init__(self, data: dict):
        self.data = data
        self.now = datetime.strptime(data["now"], "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
        self.phase = "normal"
        self.calls = 0
        self.release = None
        self.package = bytes(range(256)) * 16384
        service = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):  # keep the console for the demo's own output
                pass

            def do_POST(self):
                length = int(self.headers.get("Content-Length") or 0)
                body = self.rfile.read(length).decode("utf-8", "replace")
                status, answer = service.answer(self.path, self.headers.get("Authorization", ""), body)
                payload = json.dumps(answer).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def do_GET(self):
                status, payload, slow = service.github(self.path)
                self.send_response(status)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                step = 64 * 1024 if slow else max(1, len(payload))
                try:
                    for start in range(0, len(payload), step):
                        self.wfile.write(payload[start : start + step])
                        if slow:
                            time.sleep(0.08)
                except (ConnectionError, OSError):
                    pass  # the app canceled the download

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *exc):
        self.server.shutdown()
        self.server.server_close()

    # --- data -----------------------------------------------------------------------------

    def account(self, name: str) -> dict | None:
        for account in self.data["accounts"]:
            if account["name"] == name:
                merged = dict(account)
                merged.update(self.data["phases"].get(self.phase, {}).get(name, {}))
                return merged
        return None

    def _bucket(self, bucket_id: str, label: str, window: str, limit) -> dict:
        fraction, reset = limit
        bucket = {"bucketId": bucket_id, "displayName": label, "window": window, "remainingFraction": fraction}
        if _after(self.now, reset):
            bucket["resetTime"] = _after(self.now, reset)
        return bucket

    # --- the pretend GitHub ---------------------------------------------------------------

    def github(self, path: str) -> tuple[int, bytes, bool]:
        base = "/gh/owner/repo/releases"
        if not self.release:
            return 404, b"", False
        version = self.release["version"]
        package = f"AntigravityAccountSwitcher-{version}-portable.zip"
        if path == f"{base}/latest/download/latest.json":
            manifest = {
                "schema": 1,
                "version": version,
                "published": self.data["now"],
                "notes": f"{self.url}{base}/tag/v{version}",
                "windows-x64": {
                    "url": f"{self.url}{base}/download/v{version}/{package}",
                    "sha256": hashlib.sha256(self.package).hexdigest(),
                    "size": len(self.package),
                    "file": "AntigravityAccountSwitcher.exe",
                },
            }
            return 200, json.dumps(manifest).encode(), False
        if path == f"{base}/download/v{version}/{package}":
            return 200, self.package, True
        return 404, b"", False

    # --- the endpoints ----------------------------------------------------------------------

    def answer(self, path: str, authorization: str, body: str) -> tuple[int, dict]:
        self.calls += 1
        if path == "/token":
            form = parse_qs(body)
            token = (form.get("refresh_token") or [""])[0]
            account = self.account(token.removeprefix("demo-refresh-"))
            if not account or not token.startswith("demo-refresh-"):
                return 400, {"error": "invalid_request", "error_description": "Unknown refresh token."}
            if account.get("expired"):
                return 400, {"error": "invalid_grant", "error_description": "Token has been expired or revoked."}
            return 200, {"access_token": f"demo-access-{account['name']}", "expires_in": 3599, "token_type": "Bearer"}

        account = self.account(authorization.removeprefix("Bearer ").removeprefix("demo-access-"))
        if not account:
            return 401, {"error": {"code": 401, "message": "Request had invalid authentication credentials."}}
        method = path.rsplit(":", 1)[-1]
        if method == "loadCodeAssist":
            plan = account.get("plan", "Free")
            answer = {
                "currentTier": {"id": "free-tier" if plan == "Free" else "standard-tier", "name": "Free" if plan == "Free" else "Standard"},
                "cloudaicompanionProject": f"demo-project-{account['name']}",
            }
            if plan != "Free":
                answer["paidTier"] = {"id": "g1-pro-tier", "name": plan}
            return 200, answer
        if method == "retrieveUserQuotaSummary":
            # Like Google: the Gemini group first, its 5-hour limit the tightest Gemini model.
            gemini_five_hour = min(account["gemini"].values(), key=lambda limit: limit[0], default=[1.0, None])
            return 200, {
                "groups": [
                    {
                        "displayName": "Gemini Models",
                        "buckets": [
                            self._bucket("gemini-weekly", "Weekly Limit Remaining", "weekly", account.get("gemini_weekly", [1.0, None])),
                            self._bucket("gemini-5h", "Five Hour Limit Remaining", "5h", gemini_five_hour),
                        ],
                    },
                    {
                        "displayName": self.data["limits_group"],
                        "buckets": [
                            self._bucket("3p-weekly", "Weekly Limit Remaining", "weekly", account["weekly"]),
                            self._bucket("3p-5h", "Five Hour Limit Remaining", "5h", account["five_hour"]),
                        ],
                    },
                ]
            }
        if method == "fetchAvailableModels":
            models = {}
            for model in self.data["models"]:
                if model.get("shared"):
                    fraction, reset = min(account["five_hour"], account["weekly"], key=lambda limit: limit[0])
                else:
                    fraction, reset = account["gemini"].get(model["id"], [1.0, None])
                quota = {"remainingFraction": fraction}
                if _after(self.now, reset):
                    quota["resetTime"] = _after(self.now, reset)
                models[model["id"]] = {
                    "displayName": model["name"],
                    "recommended": bool(model.get("recommended")),
                    "quotaInfo": quota,
                }
            order = [model["id"] for model in self.data["models"]]
            return 200, {"agentModelSorts": [{"groups": [{"modelIds": order}]}], "models": models}
        return 404, {"error": {"code": 404, "message": f"No method {method}."}}
