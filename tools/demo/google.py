"""A pretend Google for the demo: the OAuth token endpoint and the three Cloud Code calls the
quota service makes (loadCodeAssist, retrieveUserQuotaSummary, fetchAvailableModels), answering
from accounts.json in the shapes the app parses (crates/quota/src/parse.rs).

Each account's refresh token is `demo-refresh-<name>` and its access token `demo-access-<name>`.
An account marked `expired` refuses its refresh token with `invalid_grant`, as Google does for a
revoked sign-in.
"""

from __future__ import annotations

import json
import re
import threading
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
            return 200, {
                "groups": [
                    {
                        "displayName": self.data["limits_group"],
                        "buckets": [
                            self._bucket("five-hour", "5 hours", "5h", account["five_hour"]),
                            self._bucket("weekly", "Weekly", "weekly", account["weekly"]),
                        ],
                    }
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
