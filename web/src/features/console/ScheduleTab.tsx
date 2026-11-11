import { useEffect, useState } from "react";
import { api, ApiError, errorMessage } from "../../api/client";
import type { Booking, BookingPatch, ScheduleEvent } from "../../api/types";
import { IconX } from "../../components/icons";
import { Scheduler, SchedulerLegend } from "../../components/Scheduler";
import { KV, Loadable, SampleBadge, Spinner, StatusChip } from "../../components/ui";
import { useDemo } from "../../lib/demo";
import { fmtDateLong, fmtStamp, fmtTimeRange, money, moneyShort, venueTimeLabel } from "../../lib/format";
import { useApi } from "../../lib/useApi";
import { useVenue } from "../../lib/venue";
import { OwnerActions } from "./InboxTab";

function SidePanel({
  ev,
  onClose,
  onChanged,
}: {
  ev: ScheduleEvent;
  onClose: () => void;
  onChanged: () => void;
}) {
  const { tz, city, card } = useVenue();
  const { epoch } = useDemo();
  const [booking, setBooking] = useState<Booking | null>(null);
  const [state, setState] = useState<"idle" | "loading" | "error">("idle");
  const [err, setErr] = useState<string | null>(null);
  const space = card.data?.spaces.find((s) => s.id === ev.space_id);

  useEffect(() => {
    setBooking(null);
    setErr(null);
    if (!ev.mine) return;
    setState("loading");
    api
      .booking(ev.id)
      .then((b) => {
        setBooking(b);
        setState("idle");
      })
      .catch((e) => {
        setState("error");
        setErr(e instanceof ApiError && e.notFound ? "This booking no longer exists." : errorMessage(e));
      });
  }, [ev.id, ev.mine, ev.starts_at, ev.space_id, ev.status, epoch]);

  return (
    <aside className="sidepanel" aria-label="Booking details">
      <header className="sidepanel__head">
        <div>
          <span className="eyebrow">{ev.mine ? "Your demo booking" : ev.sample ? "Sample booking" : "Booking"}</span>
          <h3 className="sidepanel__title">{ev.title}</h3>
        </div>
        <button type="button" className="icon-btn" onClick={onClose} aria-label="Close details">
          <IconX size={16} />
        </button>
      </header>
      <div className="sidepanel__chips">
        <StatusChip status={ev.status} />
        <SampleBadge show={ev.sample} />
      </div>
      <KV
        items={[
          ["Space", space?.name ?? "—"],
          ["Date", fmtDateLong(ev.starts_at, tz)],
          ["Time", `${fmtTimeRange(ev.starts_at, ev.ends_at, tz)} (${venueTimeLabel(tz, city)})`],
          ...(booking
            ? ([
                ["Quote", moneyShort(booking.quote_cents)],
                ["Deposit", moneyShort(booking.deposit_cents)],
                ["Guests", booking.headcount !== null ? String(booking.headcount) : "—"],
                ["Genre", booking.genre ?? "—"],
                ["Ticket price", booking.ticket_price_cents !== null ? money(booking.ticket_price_cents) : "—"],
              ] as [string, string][])
            : []),
        ]}
      />
      {!ev.mine && (
        <p className="muted small">
          {ev.sample
            ? "Shared fictional sample data. Only bookings from your demo session can be moved or approved."
            : "Another visitor's booking."}
        </p>
      )}
      {state === "loading" && <Spinner label="Loading booking…" />}
      {err && <p className="paypal__error">{err}</p>}
      {booking && (
        <>
          {booking.inquiry_text && (
            <blockquote className="inquiry">
              <span className="inquiry__label">Inquiry</span>
              {booking.inquiry_text}
            </blockquote>
          )}
          <OwnerActions
            booking={booking}
            onDone={(b) => {
              setBooking(b);
              onChanged();
            }}
          />
          {(booking.status === "ai_proposed" || booking.status === "hold") && (
            <p className="hint">Drag the bar to another day or space, or drag its edges to change the days.</p>
          )}
          {booking.timeline.length > 0 && (
            <ol className="timeline timeline--compact">
              {booking.timeline.map((t, i) => (
                <li className="timeline__item" key={i}>
                  <span className="timeline__dot" aria-hidden />
                  <span className="timeline__label">{t.label}</span>
                  <time className="timeline__at">{fmtStamp(t.at, tz)}</time>
                </li>
              ))}
            </ol>
          )}
        </>
      )}
    </aside>
  );
}

export function ScheduleTab({ onInboxChange }: { onInboxChange: () => void }) {
  const { slug, city } = useVenue();
  const { epoch } = useDemo();
  const schedule = useApi(() => api.schedule(slug), [slug, epoch]);
  const [selected, setSelected] = useState<ScheduleEvent | null>(null);

  // Keep the side panel in sync with refetched data.
  useEffect(() => {
    if (!selected || !schedule.data) return;
    const fresh = schedule.data.events.find((e) => e.id === selected.id);
    if (!fresh) setSelected(null);
    else if (fresh !== selected) setSelected(fresh);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [schedule.data]);

  const onMove = async (ev: ScheduleEvent, patch: BookingPatch) => {
    await api.patchBooking(ev.id, patch);
    schedule.reload();
    onInboxChange();
  };

  return (
    <div className={`schedule-tab${selected ? " has-panel" : ""}`}>
      <section className="card sched-card">
        <header className="card__head">
          <div>
            <h2 className="card__title">Schedule</h2>
            <p className="card__subtitle">
              Click a bar for details. Your own proposals and holds can be dragged to another day or space.
            </p>
          </div>
          <SchedulerLegend editable />
        </header>
        <Loadable state={schedule} what="The schedule" endpoint={`GET /api/venues/${slug}/schedule`}>
          {(s) => (
            <Scheduler
              schedule={s}
              editable
              city={city}
              selectedId={selected?.id ?? null}
              onEventClick={setSelected}
              onMove={onMove}
              rowHeight={66}
              height={340}
            />
          )}
        </Loadable>
      </section>
      {selected && (
        <SidePanel
          ev={selected}
          onClose={() => setSelected(null)}
          onChanged={() => {
            schedule.reload();
            onInboxChange();
          }}
        />
      )}
    </div>
  );
}
