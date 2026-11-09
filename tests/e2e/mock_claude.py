"""Scripted stand-in for the Claude Messages API, for local tests of the agent loop only.
Usage: mock_claude.py <port> [request_log.jsonl]
Logs every request (selected headers + raw body) so a test can check what the server sent."""
import datetime
import http.server
import json
import os
import re
import sys
import uuid

LOG = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(os.path.abspath(__file__)), "mock_requests.jsonl")
UUID_RE = r"booking_id ([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})"


def thinking():
    return {"type": "thinking", "thinking": "", "signature": "sig-" + uuid.uuid4().hex}


def message(content, stop):
    return {
        "id": "msg_" + uuid.uuid4().hex[:12], "type": "message", "role": "assistant",
        "model": "claude-opus-5-5", "content": content, "stop_reason": stop, "stop_details": None,
        "usage": {"input_tokens": 120, "output_tokens": 60, "cache_read_input_tokens": 1500,
                  "cache_creation_input_tokens": 300},
    }


def tool_use(name, inp):
    return message([thinking(), {"type": "tool_use", "id": "toolu_" + uuid.uuid4().hex[:16],
                                  "name": name, "input": inp}], "tool_use")


def text(t):
    return message([thinking(), {"type": "text", "text": t}], "end_turn")


def respond(body):
    msgs = body["messages"]
    last = msgs[-1]["content"]
    if last and last[0].get("type") == "tool_result":
        prev = msgs[-2]["content"]
        tu = next(b for b in prev if b["type"] == "tool_use")
        raw = last[0]["content"]
        result = json.loads(raw) if raw.startswith("{") else {"raw": raw}
        if tu["name"] == "check_availability":
            slots = result.get("slots", [])[:2]
            if not slots:
                return text("(mock) No open Friday found.")
            return tool_use("propose_slots", {
                "title": "Synthwave Night", "genre": "Synthwave", "headcount": 300, "artist": "Hollow Arcade",
                "slots": [{"space": s["space"], "date": s["date"], "start_time": "19:00", "hours": 4} for s in slots]})
        flag = " (is_error)" if last[0].get("is_error") else ""
        return text(f"(mock) {tu['name']}{flag}: {json.dumps(result)[:300]}")
    context = last[0]["text"]
    user = last[-1]["text"].lower()
    if "pay" in user:
        m = re.search(r"This conversation's booking: .*?" + UUID_RE, context)
        return tool_use("create_deposit_order", {"booking_id": m.group(1)}) if m else text("(mock) no booking yet")
    if "book" in user:
        m = re.search(r"Open proposals: .*?" + UUID_RE, context)
        return tool_use("request_booking", {"booking_id": m.group(1), "inquiry_text": "Synthwave night, ~300 guests, needs a DJ booth."}) if m else text("(mock) no proposal")
    today = datetime.date.today()
    return tool_use("check_availability", {"from_date": str(today + datetime.timedelta(days=1)),
                                           "to_date": str(today + datetime.timedelta(days=40)),
                                           "weekdays": ["Fri"], "headcount": 300})


class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        raw = self.rfile.read(int(self.headers["content-length"])).decode()
        body = json.loads(raw)
        keep = ("anthropic-version", "anthropic-beta", "content-type")
        with open(LOG, "a") as f:
            f.write(json.dumps({"headers": {k.lower(): v for k, v in self.headers.items() if k.lower() in keep},
                                "has_key": bool(self.headers.get("x-api-key")), "raw": raw}) + "\n")
        out = json.dumps(respond(body)).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)

    def log_message(self, *a):
        pass


if __name__ == "__main__":
    open(LOG, "w").close()
    http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
