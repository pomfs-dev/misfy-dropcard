import { useState } from "react";
import { Link } from "react-router-dom";
import { api, errorMessage } from "../../api/client";
import type { Booking } from "../../api/types";
import { IconCheck } from "../../components/icons";
import { EmptyState, InlineConfirm, KV, Loadable, StatusChip } from "../../components/ui";
import { useDemo } from "../../lib/demo";
import { fmtDateLong, fmtStamp, fmtTimeRange, moneyShort } from "../../lib/format";
import type { ApiState } from "../../lib/useApi";
import { useVenue } from "../../lib/venue";

const CREATED_BY: Record<string, string> = {
  promoter: "Promoter (booking form)",
  agent: "AI concierge",
  owner: "Venue owner",
  sample: "Sample",
};

/** Approve / Decline for holds; a calm note for approved bookings waiting for the deposit. */
export function OwnerActions({ booking, onDone }: { booking: Booking; onDone: (b: Booking) => void }) {
  const { bump } = useDemo();
  const [busy, setBusy] = useState<null | "approve" | "decline">(null);
  const [err, setErr] = useState<string | null>(null);

  if (booking.status === "approved") {
    return <p className="note note--blue">Approved — waiting for the deposit.</p>;
  }
  if (booking.status !== "hold") return null;

  const act = async (kind: "approve" | "decline") => {
    setBusy(kind);
    setErr(null);
    try {
      const b = kind === "approve" ? await api.approve(booking.id) : await api.decline(booking.id);
      onDone(b);
      bump();
    } catch (e) {
      setErr(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="owner-actions">
      <button type="button" className="btn btn--primary" disabled={busy !== null} onClick={() => act("approve")}>
        <IconCheck size={15} /> {busy === "approve" ? "Approving…" : "Approve"}
      </button>
      <InlineConfirm
        label="Decline"
        question="Decline this request?"
        confirmLabel="Decline"
        className="btn btn--ghost"
        busy={busy === "decline"}
        onConfirm={() => act("decline")}
      />
      {err && (
        <p className="paypal__error" role="alert">
          {err}
        </p>
      )}
    </div>
  );
}

function InboxCard({ b, tz, city, onDone }: { b: Booking; tz: number; city: string | null; onDone: (b: Booking) => void }) {
  return (
    <article className={`inbox-card inbox-card--${b.status}`}>
      <header className="inbox-card__head">
        <div>
          <h3 className="inbox-card__title">{b.title}</h3>
          <p className="inbox-card__when">
            {b.space_name} · {fmtDateLong(b.starts_at, tz)} · {fmtTimeRange(b.starts_at, b.ends_at, tz)}
            {city ? ` (${city} time)` : ""}
          </p>
        </div>
        <StatusChip status={b.status} />
      </header>
      <KV
        items={[
          ["Quote", moneyShort(b.quote_cents)],
          ["Deposit", moneyShort(b.deposit_cents)],
          ["Guests", b.headcount !== null ? String(b.headcount) : "—"],
          ["Genre", b.genre ?? "—"],
          ["Created by", CREATED_BY[b.created_by] ?? b.created_by],
        ]}
      />
      {b.inquiry_text && (
        <blockquote className="inquiry">
          <span className="inquiry__label">Inquiry</span>
          {b.inquiry_text}
        </blockquote>
      )}
      {b.timeline.length > 0 && (
        <p className="muted small">
          Last update: {b.timeline[b.timeline.length - 1].label} · {fmtStamp(b.timeline[b.timeline.length - 1].at, tz)}
        </p>
      )}
      <OwnerActions booking={b} onDone={onDone} />
    </article>
  );
}

export function InboxTab({ inbox }: { inbox: ApiState<Booking[]> }) {
  const { slug, tz, city } = useVenue();
  return (
    <Loadable state={inbox} what="The inbox" endpoint={`GET /api/console/${slug}/inbox`}>
      {(rows) =>
        rows.length === 0 ? (
          <EmptyState title="No requests waiting">
            When a promoter requests a slot in the <Link to={`/v/${slug}`}>Promoter view</Link>, it shows up here for
            approval.
          </EmptyState>
        ) : (
          <div className="inbox">
            {rows.map((b) => (
              <InboxCard
                key={b.id}
                b={b}
                tz={tz}
                city={city}
                onDone={(nb) =>
                  inbox.setData(
                    rows.flatMap((x) => (x.id === nb.id ? (["hold", "approved"].includes(nb.status) ? [nb] : []) : [x])),
                  )
                }
              />
            ))}
          </div>
        )
      }
    </Loadable>
  );
}
