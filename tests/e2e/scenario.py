"""End-to-end scenario: AI concierge -> venue approval -> PayPal deposit (confirmed by webhook) -> tickets ->
demo clock -> settlement -> PayPal Payouts, plus checks on what the server sent to the Claude API.
Runs against a server wired to mock_claude.py and mock_paypal.py (see run.sh). Standard library only.
Usage: scenario.py <app_url> <mock_paypal_url> <claude_request_log>"""
import http.cookiejar
import json
import sys
import time
import urllib.error
import urllib.request
import uuid

APP, PAYPAL, CLAUDE_LOG = sys.argv[1], sys.argv[2], sys.argv[3]
VENUE = "blue-lantern"
opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
FAILED = []


def call(method, path, body=None, headers=None, base=None):
    data = json.dumps(body).encode() if body is not None else None
    h = {"content-type": "application/json"} if data is not None else {}
    h.update(headers or {})
    req = urllib.request.Request((base or APP) + path, data=data, headers=h, method=method)
    try:
        with opener.open(req, timeout=60) as r:
            return r.status, json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            return e.code, json.loads(raw or b"null")
        except ValueError:
            return e.code, raw.decode(errors="replace")


def check(name, ok, detail=""):
    print(("PASS  " if ok else "FAIL  ") + name + ("" if ok or not detail else f"   -> {detail}"))
    if not ok:
        FAILED.append(name)


def wait(fn, seconds=20):
    end = time.time() + seconds
    while time.time() < end:
        value = fn()
        if value:
            return value
        time.sleep(0.5)
    return None


def chat(message, conversation_id=None):
    body = {"venue_slug": VENUE, "message": message}
    if conversation_id:
        body["conversation_id"] = conversation_id
    return call("POST", "/api/agent/chat", body)[1]


def booking(bid):
    return call("GET", f"/api/bookings/{bid}")[1]


call("GET", "/api/session")
call("POST", "/api/demo/reset")

# 1. AI concierge
t1 = chat("A Friday night in the next few weeks for about 300 people, synthwave.")
check("agent checks availability and proposes slots",
      [c["name"] for c in t1.get("tool_calls", [])] == ["check_availability", "propose_slots"] and len(t1.get("proposals", [])) >= 1, t1)
conv = t1["conversation_id"]
t2 = chat("Book the first one please.", conv)
check("agent sends the booking request (hold)", bool(t2.get("booking")) and t2["booking"]["status"] == "hold", t2)
bid = t2["booking"]["id"]
t3 = chat("Can I pay the deposit now?", conv)
check("payment tool is blocked by the server before approval", any(c["status"] == "blocked" for c in t3.get("tool_calls", [])), t3)

# 2. Venue owner approves
status, b = call("POST", f"/api/console/bookings/{bid}/approve")
check("venue owner approves", status == 200 and b["status"] == "approved", b)

# 3. Deposit with PayPal Orders v2
_, o1 = call("POST", f"/api/bookings/{bid}/deposit-order", headers={"Idempotency-Key": str(uuid.uuid4())})
_, o2 = call("POST", f"/api/bookings/{bid}/deposit-order", headers={"Idempotency-Key": str(uuid.uuid4())})
check("a double click creates one PayPal order", o1.get("paypal_order_id") and o1["paypal_order_id"] == o2.get("paypal_order_id"), (o1, o2))
_, cap = call("POST", f"/api/paypal/orders/{o1['paypal_order_id']}/capture")
check("capture does not confirm the booking by itself", cap["status"] == "pending_webhook" and cap["booking"]["status"] == "approved", cap)
paid = wait(lambda: (lambda d: d if d["status"] == "paid" else None)(booking(bid)))
check("the verified webhook confirms the deposit", bool(paid) and paid["deposit_order"]["confirmed_by"] == "webhook", paid)
_, r = call("POST", "/redeliver", base=PAYPAL)
check("a redelivered webhook is ignored", r.get("app_status") == 200 and r.get("app_body") == "duplicate", r)
_, r = call("POST", "/forge", base=PAYPAL)
check("a webhook with a forged signature is rejected", r.get("app_status") == 400, r)

# 4. Tickets
key = str(uuid.uuid4())
_, t = call("POST", f"/api/shows/{bid}/ticket-order", {"quantity": 2}, {"Idempotency-Key": key})
_, t_again = call("POST", f"/api/shows/{bid}/ticket-order", {"quantity": 2}, {"Idempotency-Key": key})
check("a ticket purchase is idempotent per key", t.get("paypal_order_id") == t_again.get("paypal_order_id") and t.get("amount_cents") == 6000, (t, t_again))
call("POST", f"/api/paypal/orders/{t['paypal_order_id']}/capture")


def ticket_captured():
    orders = call("GET", f"/api/console/{VENUE}/payments")[1]["orders"]
    return any(o["paypal_order_id"] == t["paypal_order_id"] and o["status"] == "captured" for o in orders)


check("the ticket payment is confirmed by webhook", bool(wait(ticket_captured)))

# 5. After the show: settlement and PayPal Payouts
call("POST", "/api/demo/clock/advance", {"hours": 168})
_, run = call("POST", f"/api/console/{VENUE}/settlement/run")
mine = [x for x in run["batches"] if not x["sample"]]
check("the settlement is planned and sent after the show", bool(mine) and mine[0]["status"] in ("submitted", "succeeded"), mine)
done = wait(lambda: (lambda d: d if d.get("settlement") and d["settlement"]["status"] == "succeeded" else None)(booking(bid)))
check("one payout batch pays venue, artist and promoter",
      bool(done) and sorted(i["role"] for i in done["settlement"]["items"]) == ["artist", "promoter", "venue"]
      and all(i["status"] == "succeeded" for i in done["settlement"]["items"]), done and done.get("settlement"))
call("POST", f"/api/console/{VENUE}/settlement/run")
_, stats = call("GET", "/stats", base=PAYPAL)
check("settling again never pays twice", stats.get("payouts") == 1, stats)
dash = call("GET", f"/api/console/{VENUE}/dashboard")[1]
row = next((s for s in dash["shows"] if s["booking_id"] == bid), None)
out = row and (row["venue_cents"] + row["artist_cents"] + row["promoter_cents"] + row["platform_fee_cents"] + row["paypal_fees_cents"])
check("every cent is accounted for (payouts + platform fee + PayPal fees = gross)", bool(row) and out == row["gross_cents"] == 78000, row)

# 6. What the server sent to the Claude API
requests = [json.loads(line) for line in open(CLAUDE_LOG)]
bodies = [json.loads(r["raw"]) for r in requests]
check("Claude API: anthropic-version 2026-10-21 and no sampling parameters",
      all(r["headers"].get("anthropic-version") == "2026-10-21" for r in requests)
      and not any(k in b for b in bodies for k in ("temperature", "top_p", "top_k")))
check("Claude API: system prompt and tools identical on every request (cacheable prefix)",
      len({json.dumps(b["system"]) + json.dumps(b["tools"]) for b in bodies}) == 1)
check("Claude API: conversation history is append-only",
      all(b2["messages"][:len(b1["messages"])] == b1["messages"] for b1, b2 in zip(bodies, bodies[1:])))

print()
print("ALL PASSED" if not FAILED else "FAILED: " + "; ".join(FAILED))
sys.exit(1 if FAILED else 0)
