"""Stand-in for the PayPal sandbox REST API, for local end-to-end tests only.
Usage: mock_paypal.py <port> <app_base_url>   (configure the app with PAYPAL_BASE_URL=http://127.0.0.1:<port>/sandbox)
Implements: OAuth token, Orders v2 create/get/capture (idempotent by PayPal-Request-Id), webhook signature
verification (SUCCESS only if the event bytes match what this mock sent), Payouts create/get, and it delivers
webhooks (PAYMENT.CAPTURE.COMPLETED, PAYMENT.PAYOUTS-ITEM.SUCCEEDED) to the app like PayPal would."""
import datetime
import http.server
import json
import sys
import threading
import urllib.request
import uuid

PORT = int(sys.argv[1])
APP = sys.argv[2].rstrip("/")
ORDERS, BY_REQ, PAYOUTS, SENT_EVENTS, WEBHOOKS = {}, {}, {}, {}, []
LOCK = threading.Lock()
STATS = {"verify_success": 0, "verify_failure": 0, "create_order": 0, "capture": 0, "payouts": 0}


def money(cents):
    return f"{cents // 100}.{cents % 100:02d}"


def cents(value):
    whole, _, frac = value.partition(".")
    return int(whole) * 100 + int((frac + "00")[:2])


def now():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def send_webhook(event_type, resource, forge=False):
    event = {"id": "WH-" + uuid.uuid4().hex[:17].upper(), "event_version": "1.0", "create_time": now(),
             "resource_type": "capture" if "CAPTURE" in event_type else "payouts_item",
             "event_type": event_type, "summary": event_type, "resource": resource}
    raw = json.dumps(event, separators=(", ", ": "))  # odd spacing on purpose: must survive byte for byte
    tid = str(uuid.uuid4())
    with LOCK:
        SENT_EVENTS[tid] = raw
    headers = {"content-type": "application/json", "paypal-auth-algo": "SHA256withRSA",
               "paypal-cert-url": "https://api.sandbox.paypal.com/v1/notifications/certs/CERT-mock",
               "paypal-transmission-id": tid, "paypal-transmission-sig": "c2lnbmF0dXJl" if not forge else "Zm9yZ2Vk",
               "paypal-transmission-time": now()}
    req = urllib.request.Request(APP + "/api/paypal/webhook", data=raw.encode(), headers=headers, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            return r.status, raw
    except urllib.error.HTTPError as e:
        return e.code, raw


def later(seconds, fn, *args):
    threading.Timer(seconds, fn, args).start()


class Handler(http.server.BaseHTTPRequestHandler):
    def reply(self, status, obj):
        out = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)

    def body(self):
        n = int(self.headers.get("content-length") or 0)
        return self.rfile.read(n).decode() if n else ""

    def do_GET(self):
        path = self.path.split("?")[0].replace("/sandbox", "", 1)
        if path.startswith("/v2/checkout/orders/"):
            oid = path.rsplit("/", 1)[1]
            return self.reply(200, ORDERS[oid]) if oid in ORDERS else self.reply(404, {"name": "RESOURCE_NOT_FOUND"})
        if path.startswith("/v1/payments/payouts/"):
            bid = path.rsplit("/", 1)[1]
            return self.reply(200, PAYOUTS[bid]) if bid in PAYOUTS else self.reply(404, {"name": "INVALID_RESOURCE_ID"})
        if path == "/stats":
            return self.reply(200, STATS)
        if path == "/v1/notifications/webhooks":
            return self.reply(200, {"webhooks": WEBHOOKS})
        self.reply(404, {"name": "NOT_FOUND"})

    def do_POST(self):
        path = self.path.split("?")[0].replace("/sandbox", "", 1)
        raw = self.body()
        rid = self.headers.get("paypal-request-id")
        if path == "/v1/oauth2/token":
            return self.reply(200, {"access_token": "A21-mock", "token_type": "Bearer", "expires_in": 32400})
        if path in ("/forge", "/redeliver"):
            return self.helper(path)
        if not (self.headers.get("authorization") or "").startswith("Bearer "):
            return self.reply(401, {"name": "AUTHENTICATION_FAILURE"})
        if rid and (path, rid) in BY_REQ:  # idempotent replay
            return self.reply(200, BY_REQ[(path, rid)])
        if path == "/v2/checkout/orders":
            b = json.loads(raw)
            pu = b["purchase_units"][0]
            oid = uuid.uuid4().hex[:17].upper()
            order = {"id": oid, "status": "CREATED", "intent": b["intent"],
                     "purchase_units": [{"reference_id": pu["reference_id"], "custom_id": pu["custom_id"],
                                         "description": pu.get("description"), "amount": pu["amount"]}]}
            ORDERS[oid] = order
            STATS["create_order"] += 1
            BY_REQ[(path, rid)] = order
            return self.reply(201, order)
        if path.startswith("/v2/checkout/orders/") and path.endswith("/capture"):
            oid = path.split("/")[4]
            order = ORDERS.get(oid)
            if not order:
                return self.reply(404, {"name": "RESOURCE_NOT_FOUND"})
            if order["status"] == "COMPLETED":
                return self.reply(422, {"name": "UNPROCESSABLE_ENTITY", "details": [{"issue": "ORDER_ALREADY_CAPTURED"}]})
            pu = order["purchase_units"][0]
            gross = cents(pu["amount"]["value"])
            fee = (gross * 349 + 5000) // 10000 + 49
            cap = {"id": uuid.uuid4().hex[:17].upper(), "status": "COMPLETED", "amount": pu["amount"],
                   "custom_id": pu["custom_id"],
                   "seller_receivable_breakdown": {"gross_amount": pu["amount"],
                                                   "paypal_fee": {"currency_code": "USD", "value": money(fee)},
                                                   "net_amount": {"currency_code": "USD", "value": money(gross - fee)}},
                   "supplementary_data": {"related_ids": {"order_id": oid}}}
            order["status"] = "COMPLETED"
            order["payer"] = {"email_address": "sb-buyer@personal.example.com", "address": {"country_code": "JP"}}
            order["purchase_units"][0]["payments"] = {"captures": [cap]}
            STATS["capture"] += 1
            BY_REQ[(path, rid)] = order
            later(0.8, send_webhook, "PAYMENT.CAPTURE.COMPLETED", cap)
            return self.reply(201, order)
        if path == "/v1/notifications/verify-webhook-signature":
            b = json.loads(raw)
            # The mock accepts only if webhook_event is exactly the bytes it sent (and the sig is not forged).
            sent = SENT_EVENTS.get(b.get("transmission_id"))
            ok = sent is not None and ('"webhook_event":' + sent) in raw and b.get("transmission_sig") == "c2lnbmF0dXJl" \
                and b.get("webhook_id") == "WH-ID-MOCK"
            STATS["verify_success" if ok else "verify_failure"] += 1
            return self.reply(200, {"verification_status": "SUCCESS" if ok else "FAILURE"})
        if path == "/v1/notifications/webhooks":
            b = json.loads(raw)
            hook = {"id": "WH-" + uuid.uuid4().hex[:17].upper(), "url": b["url"], "event_types": b["event_types"]}
            WEBHOOKS.append(hook)
            return self.reply(201, hook)
        if path == "/v1/payments/payouts":
            b = json.loads(raw)
            sb = b["sender_batch_header"]["sender_batch_id"]
            if any(p["batch_header"]["sender_batch_header"]["sender_batch_id"] == sb for p in PAYOUTS.values()):
                return self.reply(400, {"name": "USER_BUSINESS_ERROR", "message": "Batch with given sender_batch_id already exists"})
            bid = uuid.uuid4().hex[:13].upper()
            items = [{"payout_item_id": uuid.uuid4().hex[:13].upper(), "transaction_status": "PENDING",
                      "payout_batch_id": bid, "payout_item": {"recipient_type": "EMAIL", "amount": i["amount"],
                                                              "receiver": i["receiver"], "sender_item_id": i["sender_item_id"]}}
                     for i in b["items"]]
            batch = {"batch_header": {"payout_batch_id": bid, "batch_status": "PENDING", "sender_batch_header": b["sender_batch_header"]},
                     "items": items}
            PAYOUTS[bid] = batch
            STATS["payouts"] += 1
            resp = {"batch_header": batch["batch_header"]}
            BY_REQ[(path, rid)] = resp

            def settle():
                for it in items:
                    it["transaction_status"] = "SUCCESS"
                    send_webhook("PAYMENT.PAYOUTS-ITEM.SUCCEEDED", it)
                batch["batch_header"]["batch_status"] = "SUCCESS"
            later(1.0, settle)
            return self.reply(201, resp)
        self.reply(404, {"name": "NOT_FOUND", "path": path})

    def helper(self, path):
        if path == "/forge":  # test helper: deliver a webhook with a forged signature
            status, _ = send_webhook("PAYMENT.CAPTURE.COMPLETED", {"id": "FORGED", "amount": {"currency_code": "USD", "value": "1.00"}}, forge=True)
            return self.reply(200, {"app_status": status})
        if path == "/redeliver":  # test helper: resend the last event exactly (PayPal retries)
            tid, raw_event = list(SENT_EVENTS.items())[-1]
            headers = {"content-type": "application/json", "paypal-auth-algo": "SHA256withRSA",
                       "paypal-cert-url": "https://api.sandbox.paypal.com/v1/notifications/certs/CERT-mock",
                       "paypal-transmission-id": tid, "paypal-transmission-sig": "c2lnbmF0dXJl", "paypal-transmission-time": now()}
            req = urllib.request.Request(APP + "/api/paypal/webhook", data=raw_event.encode(), headers=headers, method="POST")
            with urllib.request.urlopen(req, timeout=20) as r:
                return self.reply(200, {"app_status": r.status, "app_body": r.read().decode()})
        self.reply(404, {"name": "NOT_FOUND", "path": path})

    def log_message(self, *a):
        pass


if __name__ == "__main__":
    http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
