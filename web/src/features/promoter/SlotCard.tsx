import { useState } from "react";
import { errorMessage } from "../../api/client";
import type { Booking, BookingSummary } from "../../api/types";
import { IconCheck } from "../../components/icons";
import { Chip, StatusChip } from "../../components/ui";
import { fmtDateLong, fmtTimeRange, moneyShort } from "../../lib/format";

/** One proposed slot (quote, deposit, request button); shared by the booking form and the optional AI concierge. */
export function SlotCard({
  p,
  tz,
  city,
  booking,
  onRequest,
}: {
  p: BookingSummary;
  tz: number;
  city: string | null;
  /** the conversation's booking, once a slot was picked */
  booking: Booking | null;
  onRequest: (p: BookingSummary) => Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const picked = booking && booking.status !== "ai_proposed" ? booking : null;
  const isRequested = picked?.id === p.id;
  const otherRequested = picked !== null && !isRequested;
  return (
    <article className={`slot${isRequested ? ` slot--picked slot--${picked?.status}` : ""}${otherRequested ? " slot--released" : ""}`}>
      <header className="slot__head">
        <span className="slot__space">{p.space_name}</span>
        {isRequested && picked ? (
          <StatusChip status={picked.status} />
        ) : otherRequested ? (
          <Chip tone="neutral">Released</Chip>
        ) : (
          <StatusChip status={p.status} />
        )}
      </header>
      <div className="slot__when">
        <strong>{fmtDateLong(p.starts_at, tz)}</strong>
        <span>
          {fmtTimeRange(p.starts_at, p.ends_at, tz)}
          {city ? ` · ${city} time` : ""}
        </span>
      </div>
      <p className="slot__money">
        <span>
          <strong>{moneyShort(p.quote_cents)}</strong> quote
        </span>
        <span>
          <strong>{moneyShort(p.deposit_cents)}</strong> deposit
        </span>
        {p.headcount !== null && (
          <span>
            <strong>{p.headcount}</strong> guests
          </span>
        )}
        {p.genre && <span>{p.genre}</span>}
      </p>
      {isRequested ? (
        <p className="slot__done">
          <IconCheck size={14} />{" "}
          {picked?.status === "hold"
            ? "Requested — waiting for the venue to approve."
            : picked?.status === "approved"
              ? "Approved — pay the deposit in “Your booking”."
              : picked?.status === "paid"
                ? "Confirmed — deposit paid."
                : "Requested."}
        </p>
      ) : otherRequested ? (
        <p className="slot__released">Released when you picked another slot.</p>
      ) : (
        <button
          type="button"
          className="btn btn--primary btn--block"
          disabled={busy || p.status !== "ai_proposed"}
          onClick={async () => {
            setBusy(true);
            setErr(null);
            try {
              await onRequest(p);
            } catch (e) {
              setErr(errorMessage(e));
            } finally {
              setBusy(false);
            }
          }}
        >
          {busy ? "Requesting…" : "Request this slot"}
        </button>
      )}
      {err && (
        <p className="slot__error" role="alert">
          {err}
        </p>
      )}
    </article>
  );
}
