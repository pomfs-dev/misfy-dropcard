import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { api, ApiError, errorMessage } from "../../api/client";
import type { Booking, ConfirmedBy, PaymentOrder } from "../../api/types";
import { IconChevron, IconShield } from "../../components/icons";
import { PayPalCheckout } from "../../components/PayPalCheckout";
import { Chip, InlineConfirm, KV, Spinner, StateChip, StatusChip } from "../../components/ui";
import { useDemo } from "../../lib/demo";
import { fmtDateLong, fmtStamp, fmtTimeRange, money, moneyShort } from "../../lib/format";
import { depositKey, uuid } from "../../lib/ids";
import { usePoll } from "../../lib/useApi";

export function confirmedByText(by: ConfirmedBy | undefined): string {
  return by === "reconciliation"
    ? "Confirmed by reconciliation with the PayPal API"
    : "Confirmed by a verified PayPal webhook";
}

type Props = {
  booking: Booking;
  slug: string;
  tz: number;
  city: string | null;
  /** fresh booking from the server (poll / action) */
  onChange: (b: Booking | null) => void;
};

function Timeline({ booking, tz }: { booking: Booking; tz: number }) {
  if (!booking.timeline.length) return null;
  return (
    <ol className="timeline">
      {booking.timeline.map((t, i) => (
        <li className="timeline__item" key={`${t.at}-${i}`}>
          <span className="timeline__dot" aria-hidden />
          <span className="timeline__label">{t.label}</span>
          <time className="timeline__at">{fmtStamp(t.at, tz)}</time>
        </li>
      ))}
    </ol>
  );
}

function SettlementSummary({ booking, tz }: { booking: Booking; tz: number }) {
  const s = booking.settlement;
  if (!s) return null;
  return (
    <div className="settle">
      <div className="settle__head">
        <span className="settle__title">Payout batch</span>
        <StateChip status={s.status} />
      </div>
      <p className="muted small">
        Due {fmtStamp(s.scheduled_for, tz)} · <code>{s.sender_batch_id}</code>
      </p>
      <ul className="settle__items">
        {s.items.map((it) => (
          <li key={it.role}>
            <span className="settle__role">{it.role}</span>
            <span className="settle__amt">{money(it.amount_cents)}</span>
            <StateChip status={it.status} />
          </li>
        ))}
      </ul>
      {s.last_error && <p className="paypal__error">{s.last_error}</p>}
    </div>
  );
}

export function BookingPanel({ booking, slug, tz, city, onChange }: Props) {
  const { epoch, bump } = useDemo();
  const [waiting, setWaiting] = useState(false);
  const [waitStarted, setWaitStarted] = useState<number | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const [actionError, setActionError] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const id = booking.id;

  // Refetch on demo clock changes.
  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    api
      .booking(id)
      .then(onChange)
      .catch((e) => e instanceof ApiError && e.notFound && onChange(null));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [epoch, id]);

  const status = booking.status;
  const settling = booking.settlement && ["planned", "submitted"].includes(booking.settlement.status);
  const pollMs = waiting ? 2000 : status === "hold" || status === "approved" ? 4000 : settling ? 5000 : 0;

  usePoll(
    () => api.booking(id),
    pollMs || 1000,
    pollMs > 0,
    (b) => {
      onChange(b);
      if (b.status === "paid" || b.status === "completed") {
        if (waiting) {
          setWaiting(false);
          bump();
        }
        return !settling;
      }
      return false;
    },
    (e) => {
      if (e instanceof ApiError && e.notFound) onChange(null);
    },
  );

  useEffect(() => {
    if (!waiting || waitStarted === null) return;
    const t = setInterval(() => setElapsed(Math.round((Date.now() - waitStarted) / 1000)), 1000);
    return () => clearInterval(t);
  }, [waiting, waitStarted]);

  const createDeposit = useCallback(async () => {
    const r = await api.createDepositOrder(id, depositKey(id));
    return r.paypal_order_id;
  }, [id]);

  const deposit = booking.deposit_order;
  const paidBy = deposit?.confirmed_by ?? null;

  return (
    <section className="card booking" aria-label="Your booking">
      <header className="booking__head">
        <span className="eyebrow">Your booking</span>
        <StatusChip status={status} />
      </header>
      <h2 className="booking__title">{booking.title}</h2>
      <p className="booking__when">
        {booking.space_name} · {fmtDateLong(booking.starts_at, tz)}
        <br />
        {fmtTimeRange(booking.starts_at, booking.ends_at, tz)}
        {city ? ` (${city} time)` : ""}
      </p>
      <KV
        items={[
          ["Quote", moneyShort(booking.quote_cents)],
          ["Deposit", moneyShort(booking.deposit_cents)],
          ...(booking.headcount !== null ? ([["Guests", String(booking.headcount)]] as [string, string][]) : []),
          ...(booking.genre ? ([["Genre", booking.genre]] as [string, string][]) : []),
          ...(booking.artist ? ([["Artist", booking.artist.name]] as [string, string][]) : []),
        ]}
      />

      <div className="booking__step">
        {status === "ai_proposed" && <p className="muted">Pick “Request this slot” to send the inquiry.</p>}

        {status === "hold" && (
          <>
            <div className="callout callout--yellow">
              <p>
                <strong>Waiting for the venue to approve.</strong> The slot is held for you while the owner decides.
              </p>
            </div>
            <div className="booking__actions">
              <Link to={`/console/${slug}?tab=inbox`} className="btn btn--secondary btn--sm">
                Approve it as the venue owner <IconChevron size={14} />
              </Link>
              <InlineConfirm
                label="Withdraw request"
                question="Withdraw this request?"
                confirmLabel="Withdraw"
                busy={cancelling}
                onConfirm={async () => {
                  setCancelling(true);
                  setActionError(null);
                  try {
                    onChange(await api.cancelBooking(id));
                    bump();
                  } catch (e) {
                    setActionError(errorMessage(e));
                  } finally {
                    setCancelling(false);
                  }
                }}
              />
            </div>
          </>
        )}

        {status === "approved" && !waiting && (
          <>
            <div className="callout callout--blue">
              <p>
                <strong>Approved by the venue.</strong> Pay the {moneyShort(booking.deposit_cents)} deposit to confirm the
                date.
              </p>
            </div>
            <PayPalCheckout
              createOrder={createDeposit}
              onCaptured={(res) => {
                onChange(res.booking);
                if (res.booking.status === "paid") {
                  bump();
                  return;
                }
                setWaiting(true);
                setWaitStarted(Date.now());
                setElapsed(0);
              }}
            />
            <p className="fineprint">
              <IconShield size={13} /> The booking is confirmed only when our server receives PayPal’s verified webhook —
              never by the browser.
            </p>
          </>
        )}

        {waiting && status !== "paid" && (
          <div className="callout callout--wait" role="status">
            <Spinner />
            <div>
              <strong>Waiting for PayPal confirmation (webhook)…</strong>
              <p className="small">
                PayPal captured the payment. The date is confirmed as soon as PayPal’s signed webhook reaches our
                server{elapsed > 3 ? ` · ${elapsed}s` : ""}.
              </p>
              {elapsed > 45 && <p className="small muted">Sandbox webhooks can take a minute. We keep checking.</p>}
            </div>
          </div>
        )}

        {(status === "paid" || status === "completed") && deposit?.status === "captured" && (
          <div className="callout callout--green" role="status">
            <IconShield size={18} />
            <div>
              <strong>
                {paidBy === "reconciliation"
                  ? "Deposit confirmed by reconciliation with the PayPal API"
                  : paidBy === "webhook"
                    ? "Deposit confirmed by a verified PayPal webhook"
                    : "Deposit paid"}
              </strong>
              <p className="small">
                {money(deposit.amount_cents)}
                {deposit.paypal_order_id ? (
                  <>
                    {" "}
                    · PayPal order <code>{deposit.paypal_order_id}</code>
                  </>
                ) : null}
              </p>
            </div>
          </div>
        )}

        {status === "cancelled" && <div className="callout">This booking was cancelled.</div>}

        {actionError && (
          <p className="paypal__error" role="alert">
            {actionError}
          </p>
        )}
      </div>

      {status === "paid" && <TicketPanel booking={booking} slug={slug} tz={tz} />}

      <SettlementSummary booking={booking} tz={tz} />

      <details className="booking__timeline" open>
        <summary>Timeline</summary>
        <Timeline booking={booking} tz={tz} />
      </details>
    </section>
  );
}

// ---- Buy tickets (fan's view)

type Purchase = { orderId: string; quantity: number; amount: number; state: "waiting" | "confirmed"; by: ConfirmedBy };

function TicketPanel({ booking, slug, tz }: { booking: Booking; slug: string; tz: number }) {
  const { nowMs, bump } = useDemo();
  const [qty, setQty] = useState(2);
  const [key, setKey] = useState(() => uuid()); // one Idempotency-Key per purchase
  const [purchase, setPurchase] = useState<Purchase | null>(null);
  const [orders, setOrders] = useState<PaymentOrder[]>([]);
  const price = booking.ticket_price_cents;
  const started = nowMs !== null && Date.parse(booking.starts_at) <= nowMs;

  const loadOrders = useCallback(async () => {
    const p = await api.payments(slug);
    const mine = p.orders.filter((o) => o.booking_id === booking.id && o.kind === "ticket");
    setOrders(mine);
    return mine;
  }, [slug, booking.id]);

  useEffect(() => {
    loadOrders().catch(() => undefined);
  }, [loadOrders]);

  usePoll(
    loadOrders,
    2000,
    purchase?.state === "waiting",
    (mine) => {
      const o = mine.find((x) => x.paypal_order_id === purchase?.orderId);
      if (o && o.status === "captured") {
        setPurchase((p) => (p ? { ...p, state: "confirmed", by: o.confirmed_by } : p));
        bump();
        return true;
      }
      return false;
    },
  );

  const sold = useMemo(
    () => orders.filter((o) => o.status === "captured").reduce((n, o) => n + o.quantity, 0),
    [orders],
  );

  const createOrder = useCallback(async () => {
    const r = await api.createTicketOrder(booking.id, qty, key);
    return r.paypal_order_id;
  }, [booking.id, qty, key]);

  return (
    <div className="tickets">
      <div className="tickets__head">
        <div>
          <span className="eyebrow">Fan’s view</span>
          <h3 className="tickets__title">Buy tickets</h3>
        </div>
        {sold > 0 && <Chip tone="green">{sold} sold</Chip>}
      </div>
      <p className="small muted">
        Ticket sales are what make the payout split visible: after the show one PayPal Payouts batch pays the venue,
        the artist and the promoter. Any PayPal sandbox buyer can pay.
      </p>

      {price === null ? (
        <p className="callout">This show has no ticket price yet.</p>
      ) : started ? (
        <p className="callout">Ticket sales are closed — the show has started.</p>
      ) : purchase?.state === "waiting" ? (
        <div className="callout callout--wait" role="status">
          <Spinner />
          <div>
            <strong>Waiting for PayPal confirmation (webhook)…</strong>
            <p className="small">
              {purchase.quantity} × ticket · {money(purchase.amount)}
            </p>
          </div>
        </div>
      ) : (
        <>
          {purchase?.state === "confirmed" && (
            <div className="callout callout--green" role="status">
              <IconShield size={18} />
              <div>
                <strong>
                  {purchase.quantity} ticket{purchase.quantity === 1 ? "" : "s"} confirmed
                </strong>
                <p className="small">{confirmedByText(purchase.by)}</p>
              </div>
            </div>
          )}
          <div className="qty" role="group" aria-label="Number of tickets">
            <span className="qty__label">Tickets</span>
            <div className="qty__stepper">
              {[1, 2, 3, 4, 5, 6].map((n) => (
                <button
                  type="button"
                  key={n}
                  className={`qty__opt${qty === n ? " is-active" : ""}`}
                  onClick={() => {
                    if (n !== qty) {
                      setQty(n);
                      setKey(uuid());
                    }
                  }}
                  aria-pressed={qty === n}
                >
                  {n}
                </button>
              ))}
            </div>
            <span className="qty__total">
              {qty} × {moneyShort(price)} = <strong>{money(price * qty)}</strong>
            </span>
          </div>
          <PayPalCheckout
            createOrder={createOrder}
            onCaptured={(res, orderId) => {
              void res;
              setPurchase({ orderId, quantity: qty, amount: price * qty, state: "waiting", by: null });
              setKey(uuid());
            }}
          />
        </>
      )}

      {orders.length > 0 && (
        <ul className="tickets__orders">
          {orders.slice(0, 5).map((o) => (
            <li key={o.id}>
              <span>
                {o.quantity} × ticket · {money(o.amount_cents)}
              </span>
              <span className="muted small">{fmtStamp(o.created_at, tz)}</span>
              <StateChip status={o.status} />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
