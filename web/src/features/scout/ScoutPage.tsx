import { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { api, ApiError } from "../../api/client";
import type { ArtistItem, Booking } from "../../api/types";
import { IconSpark } from "../../components/icons";
import { StatusChip } from "../../components/ui";
import { useChatStore } from "../../lib/chatStore";
import { useDemo } from "../../lib/demo";
import { fmtDateLong, fmtTimeRange, moneyShort } from "../../lib/format";
import { ConciergeChat, scoutKey } from "../promoter/ConciergeChat";
import { ACTIVITY_HELP } from "./VenueHitCard";

const ARTIST_KEY = "dropcard.scout.artist";
const DEFAULT_ARTIST = "hollow-arcade";

function readArtist(): string {
  try {
    return window.localStorage.getItem(ARTIST_KEY) || DEFAULT_ARTIST;
  } catch {
    return DEFAULT_ARTIST;
  }
}

function saveArtist(slug: string) {
  try {
    window.localStorage.setItem(ARTIST_KEY, slug);
  } catch {
    /* private mode: the choice just won't survive a reload */
  }
}

/** The booking the scout sent, with the next step (approve as the venue, then pay on the venue's card). */
function ScoutBooking({ booking }: { booking: Booking }) {
  const tz = booking.venue_tz;
  const next =
    booking.status === "hold" ? (
      <>
        Waiting for {booking.venue_name}. <Link to={`/console/${booking.venue_slug}`}>Approve it as the venue owner →</Link>
      </>
    ) : booking.status === "approved" ? (
      <>
        Approved. <Link to={`/v/${booking.venue_slug}`}>Pay the 30% deposit with PayPal on the venue’s card →</Link>
      </>
    ) : booking.status === "paid" ? (
      <>
        Deposit paid — tickets are on sale on <Link to={`/v/${booking.venue_slug}`}>the venue’s card</Link>. PayPal Payouts
        pays the venue, you and the promoter the day after the show.
      </>
    ) : booking.status === "completed" ? (
      <>Paid out to the venue, the artist and the promoter.</>
    ) : (
      <>This booking was {booking.status}.</>
    );
  return (
    <section className="card scout-booking" aria-label="Your booking">
      <header className="card__head">
        <h2 className="card__title">Your booking</h2>
        <StatusChip status={booking.status} />
      </header>
      <p className="scout-booking__title">
        <strong>{booking.title}</strong> at <Link to={`/v/${booking.venue_slug}`}>{booking.venue_name}</Link>
      </p>
      <p className="muted small">
        {fmtDateLong(booking.starts_at, tz)} · {fmtTimeRange(booking.starts_at, booking.ends_at, tz)} venue time ·{" "}
        {moneyShort(booking.quote_cents)} quote · {moneyShort(booking.deposit_cents)} deposit
      </p>
      <p className="scout-booking__next">{next}</p>
      {booking.inquiry_text && (
        <details className="scout-booking__letter">
          <summary>The proposal the AI sent</summary>
          <p>{booking.inquiry_text}</p>
        </details>
      )}
    </section>
  );
}

/** For artists: the AI venue scout finds the most active venues, checks dates and sends the proposal. */
export function ScoutPage() {
  const { config, epoch } = useDemo();
  const aiOn = config?.value?.llm_enabled === true;
  const store = useChatStore();
  const [artists, setArtists] = useState<ArtistItem[]>([]);
  const [artist, setArtist] = useState<string>(readArtist);
  const key = scoutKey(artist);
  const conv = store.get(key);
  const artistName = artists.find((a) => a.slug === artist)?.name ?? "your artist";

  useEffect(() => {
    document.title = "Find a venue · MiSFY DropCard";
    let cancelled = false;
    api
      .artists()
      .then((list) => {
        if (cancelled) return;
        setArtists(list);
        if (list.length && !list.some((a) => a.slug === artist)) setArtist(list[0].slug);
      })
      .catch(() => {
        /* the picker just stays on the default artist */
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // The venue owner approves on another page: refresh the booking when the visitor comes back.
  const bookingId = conv.booking?.id ?? store.storedBookingId(key);
  const refresh = useCallback(() => {
    if (!bookingId) return;
    api
      .booking(bookingId)
      .then((b) => store.update(key, (c) => ({ ...c, booking: b })))
      .catch((e) => {
        if (e instanceof ApiError && e.notFound) store.update(key, (c) => ({ ...c, booking: null }));
      });
  }, [bookingId, key, store]);
  useEffect(() => {
    refresh();
    window.addEventListener("focus", refresh);
    return () => window.removeEventListener("focus", refresh);
  }, [refresh, epoch]);

  const onBooking = useCallback((b: Booking) => store.update(key, (c) => ({ ...c, booking: b })), [key, store]);

  return (
    <div className="scout-grid">
      <aside className="scout-side">
        <section className="hero scout-hero" aria-label="Find a venue">
          <div className="hero__top">
            <span className="hero__eyebrow">For artists · AI venue scout</span>
          </div>
          <h1 className="hero__name">Find a venue</h1>
          <p className="hero__tagline">
            Tell the AI where you want to play. It finds the venues with the busiest calendars in MiSFY’s discovery data,
            checks their dates and sends your proposal. The venue approves, you pay the deposit with PayPal, tickets go on
            sale, and PayPal Payouts pays everyone the day after the show.
          </p>
          <ol className="scout-steps">
            <li>
              <strong>Ask</strong> where, when and for how many
            </li>
            <li>
              <strong>Compare</strong> venues ranked by activity
            </li>
            <li>
              <strong>Propose</strong> — the AI writes and sends it
            </li>
            <li>
              <strong>Approve</strong> as the venue owner
            </li>
            <li>
              <strong>Pay</strong> the 30% deposit with PayPal
            </li>
            <li>
              <strong>Settle</strong> with one PayPal Payouts batch
            </li>
          </ol>
        </section>

        <section className="card scout-artist">
          <label className="field" htmlFor="scout-artist">
            <span className="field__label">You are</span>
            <select
              id="scout-artist"
              className="field__input"
              value={artist}
              onChange={(e) => {
                setArtist(e.target.value);
                saveArtist(e.target.value);
              }}
            >
              {artists.length === 0 && <option value={artist}>{artist}</option>}
              {artists.map((a) => (
                <option key={a.slug} value={a.slug}>
                  {a.name} · {a.genre} · {a.country}
                </option>
              ))}
            </select>
          </label>
          <p className="muted small">The scout works for this artist: they headline what it books. Each artist has their own chat.</p>
        </section>

        {conv.booking && conv.booking.status !== "ai_proposed" && <ScoutBooking booking={conv.booking} />}

        <p className="muted small scout-note" title={ACTIVITY_HELP}>
          Activity ranks venues by how many shows MiSFY’s discovery data has on record for them — a busy calendar, not live
          popularity. City, genres and show counts are pseudonymized and rounded; venue names, rooms and prices are made up
          for the demo.
        </p>
      </aside>

      <div className="scout-main">
        {aiOn ? (
          <ConciergeChat mode={{ kind: "scout", artist, artistName }} onTurn={() => undefined} onBooking={onBooking} />
        ) : (
          <section className="card chat chat--scout" aria-label="AI venue scout">
            <header className="card__head">
              <h2 className="card__title">
                <span className="chat__badge" aria-hidden>
                  <IconSpark size={15} />
                </span>
                AI venue scout
              </h2>
            </header>
            <p className="chat__notlive">
              The AI venue scout is not switched on for this server (it needs an Anthropic API key). You can still book any venue
              with the booking form on its card, for example <Link to="/v/blue-lantern">Blue Lantern Hall</Link>.
            </p>
          </section>
        )}
      </div>
    </div>
  );
}
