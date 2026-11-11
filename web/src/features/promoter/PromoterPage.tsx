import { useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError } from "../../api/client";
import type { Booking, BookingSummary, ChatTurn } from "../../api/types";
import { Scheduler, SchedulerLegend } from "../../components/Scheduler";
import { ErrorNote, Loadable, Spinner } from "../../components/ui";
import { useChatStore } from "../../lib/chatStore";
import { useDemo } from "../../lib/demo";
import { venueTimeLabel } from "../../lib/format";
import { useApi } from "../../lib/useApi";
import { useVenue } from "../../lib/venue";
import { BookingFinder } from "./BookingFinder";
import { BookingPanel } from "./BookingPanel";
import { ConciergeChat } from "./ConciergeChat";
import { VenueDetails, VenueHero } from "./VenueCard";

export function PromoterPage() {
  const { slug, card, tz, city } = useVenue();
  const { epoch, config } = useDemo();
  // The AI concierge runs only when the server has an AI key; the hosted demo uses the booking form.
  const aiOn = config?.value?.llm_enabled === true;
  const store = useChatStore();
  const conv = store.get(slug);
  const schedule = useApi(() => api.schedule(slug), [slug, epoch]);
  const [highlight, setHighlight] = useState<string[]>([]);
  const lastStatus = useRef<string | null>(conv.booking?.status ?? null);

  useEffect(() => {
    if (card.data) document.title = `${card.data.venue.name} · MiSFY DropCard`;
  }, [card.data]);

  // After a reload, bring back the booking this visitor was working on (bookings are per session).
  useEffect(() => {
    if (conv.booking) return;
    const id = store.storedBookingId(slug);
    if (!id) return;
    let cancelled = false;
    api
      .booking(id)
      .then((b) => !cancelled && store.update(slug, (c) => ({ ...c, booking: b, conversationId: c.conversationId ?? b.conversation_id })))
      .catch((e) => {
        if (!cancelled && e instanceof ApiError && e.notFound) store.update(slug, (c) => ({ ...c, booking: null }));
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slug, store.storedBookingId]);

  const onTurn = useCallback(
    (turn: ChatTurn) => {
      setHighlight(turn.proposals.map((p) => p.id));
      schedule.reload();
    },
    [schedule],
  );

  const onProposals = useCallback(
    (proposals: BookingSummary[]) => {
      setHighlight(proposals.map((p) => p.id));
      schedule.reload();
    },
    [schedule],
  );

  const setBooking = useCallback(
    (b: Booking | null) => {
      store.update(slug, (c) => ({ ...c, booking: b }));
      const status = b?.status ?? null;
      if (status !== lastStatus.current) {
        lastStatus.current = status;
        schedule.reload();
      }
    },
    [slug, store, schedule],
  );

  if (card.loading && !card.data) {
    return (
      <div className="page-loading">
        <Spinner label="Loading the venue…" />
      </div>
    );
  }
  if (!card.data) {
    const notFound = card.error instanceof ApiError && card.error.notFound;
    return (
      <div className="page-narrow">
        {notFound ? (
          <div className="empty">
            <p className="empty__title">Venue not found</p>
            <p className="empty__body">There is no venue at “{slug}”.</p>
          </div>
        ) : (
          <ErrorNote error={card.error ?? "Could not load the venue."} onRetry={card.reload} />
        )}
      </div>
    );
  }

  const c = card.data;
  return (
    <div className="promo-grid">
      <aside className="promo-side">
        <VenueHero card={c} />
        {conv.booking && (
          <BookingPanel booking={conv.booking} slug={slug} tz={tz} city={city} onChange={setBooking} />
        )}
        <VenueDetails card={c} tz={tz} />
      </aside>

      <div className="promo-main">
        {aiOn ? (
          <ConciergeChat
            slug={slug}
            tz={tz}
            city={city}
            venueName={c.venue.name}
            onTurn={onTurn}
            onBooking={setBooking}
          />
        ) : (
          <BookingFinder
            slug={slug}
            tz={tz}
            city={city}
            venueName={c.venue.name}
            onProposals={onProposals}
            onBooking={setBooking}
          />
        )}

        <section className="card sched-card">
          <header className="card__head">
            <h2 className="card__title">
              Availability <span className="card__title-note">{venueTimeLabel(tz, city)} · proposals don’t reserve the date</span>
            </h2>
            <SchedulerLegend />
          </header>
          <Loadable state={schedule} what="The schedule" endpoint={`GET /api/venues/${slug}/schedule`}>
            {(s) => <Scheduler schedule={s} highlightIds={highlight} city={city} height={296} />}
          </Loadable>
        </section>
      </div>
    </div>
  );
}
