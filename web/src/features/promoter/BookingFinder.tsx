import { useEffect, useMemo, useState, type FormEvent } from "react";
import { api, errorMessage } from "../../api/client";
import type { ArtistItem, Booking, BookingSummary } from "../../api/types";
import { useChatStore } from "../../lib/chatStore";
import { useDemo } from "../../lib/demo";
import { SlotCard } from "./SlotCard";

const NIGHTS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] as const;
const HOURS = [2, 3, 4, 5, 6, 8];

type Props = {
  slug: string;
  /** venue UTC offset in minutes */
  tz: number;
  city: string | null;
  venueName: string;
  /** after a search (refetch the schedule, highlight the proposals) */
  onProposals: (proposals: BookingSummary[]) => void;
  /** after "Request this slot" */
  onBooking: (b: Booking) => void;
};

/** A date in the venue's calendar, `days` after the demo clock's today (YYYY-MM-DD). */
function venueDay(nowMs: number, tz: number, days: number) {
  return new Date(nowMs + tz * 60_000 + days * 86_400_000).toISOString().slice(0, 10);
}

/**
 * The booking form: dates, nights, crowd size and the show. The server checks every space and saves up
 * to three open slots as proposals (dashed bars); nothing is reserved until the promoter requests one.
 */
export function BookingFinder({ slug, tz, city, venueName, onProposals, onBooking }: Props) {
  const store = useChatStore();
  const conv = store.get(slug);
  const { nowMs } = useDemo();
  const today = nowMs ?? Date.now();

  const [from, setFrom] = useState(() => venueDay(today, tz, 1));
  const [to, setTo] = useState(() => venueDay(today, tz, 14));
  const [nights, setNights] = useState<string[]>(["Fri", "Sat"]);
  const [guests, setGuests] = useState("300");
  const [doors, setDoors] = useState("19:00");
  const [hours, setHours] = useState(4);
  const [title, setTitle] = useState("");
  const [genre, setGenre] = useState("");
  const [artist, setArtist] = useState("");
  const [message, setMessage] = useState("");
  const [artists, setArtists] = useState<ArtistItem[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .artists()
      .then((list) => !cancelled && setArtists(list))
      .catch(() => {
        /* the headliner list is optional */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const minDay = useMemo(() => venueDay(today, tz, 0), [today, tz]);

  const toggleNight = (n: string) =>
    setNights((cur) => (cur.includes(n) ? cur.filter((x) => x !== n) : NIGHTS.filter((x) => cur.includes(x) || x === n)));

  const onSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const headcount = Number.parseInt(guests, 10);
      const res = await api.proposals(slug, {
        from_date: from,
        to_date: to,
        weekdays: nights,
        headcount: Number.isFinite(headcount) && headcount > 0 ? headcount : undefined,
        start_time: doors || undefined,
        hours,
        title: title.trim() || undefined,
        genre: genre.trim() || undefined,
        artist: artist || undefined,
      });
      store.update(slug, (c) => ({ ...c, proposals: res.proposals, freshProposalIds: res.proposals.map((p) => p.id) }));
      setNote(res.note);
      onProposals(res.proposals);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const requestSlot = async (p: BookingSummary) => {
    const b = await api.requestBooking(p.id, message.trim() || undefined);
    store.update(slug, (c) => ({ ...c, booking: b }));
    onBooking(b);
  };

  return (
    <section className="card finder" aria-label="Book a date">
      <header className="card__head">
        <div>
          <h2 className="card__title">Book a date</h2>
          <p className="card__subtitle">
            Tell {venueName} what you need. Every space is checked and up to three open slots come back with the quote and
            the 30% deposit. Nothing is reserved until you request one.
          </p>
        </div>
      </header>

      <form className="finder__form" onSubmit={onSubmit}>
        <div className="finder__row">
          <label className="field" htmlFor="finder-from">
            <span className="field__label">First date</span>
            <input id="finder-from" className="field__input" type="date" value={from} min={minDay} onChange={(e) => setFrom(e.target.value)} required />
          </label>
          <label className="field" htmlFor="finder-to">
            <span className="field__label">Last date</span>
            <input id="finder-to" className="field__input" type="date" value={to} min={from} onChange={(e) => setTo(e.target.value)} required />
          </label>
          <label className="field field--narrow" htmlFor="finder-guests">
            <span className="field__label">Guests</span>
            <input id="finder-guests" className="field__input" type="number" inputMode="numeric" min={1} max={5000} value={guests} onChange={(e) => setGuests(e.target.value)} />
          </label>
        </div>

        <div className="finder__nights" role="group" aria-label="Nights">
          <span className="field__label">Nights</span>
          <div className="qty__stepper">
            {NIGHTS.map((n) => (
              <button key={n} type="button" className={`qty__opt finder__night${nights.includes(n) ? " is-active" : ""}`} aria-pressed={nights.includes(n)} onClick={() => toggleNight(n)}>
                {n}
              </button>
            ))}
          </div>
          <span className="muted small">{nights.length === 0 ? "Any night" : ""}</span>
        </div>

        <div className="finder__row">
          <label className="field field--narrow" htmlFor="finder-doors">
            <span className="field__label">Doors</span>
            <input id="finder-doors" className="field__input" type="time" step={3600} value={doors} onChange={(e) => setDoors(e.target.value)} />
          </label>
          <label className="field field--narrow" htmlFor="finder-hours">
            <span className="field__label">Hours</span>
            <select id="finder-hours" className="field__input" value={hours} onChange={(e) => setHours(Number(e.target.value))}>
              {HOURS.map((h) => (
                <option key={h} value={h}>
                  {h}
                </option>
              ))}
            </select>
          </label>
          <label className="field" htmlFor="finder-title">
            <span className="field__label">Show title</span>
            <input id="finder-title" className="field__input" type="text" maxLength={120} placeholder="e.g. Synthwave Night" value={title} onChange={(e) => setTitle(e.target.value)} />
          </label>
        </div>

        <div className="finder__row">
          <label className="field" htmlFor="finder-genre">
            <span className="field__label">Genre</span>
            <input id="finder-genre" className="field__input" type="text" maxLength={60} placeholder="e.g. Synthwave" value={genre} onChange={(e) => setGenre(e.target.value)} />
          </label>
          <label className="field" htmlFor="finder-artist">
            <span className="field__label">Headliner</span>
            <select id="finder-artist" className="field__input" value={artist} onChange={(e) => setArtist(e.target.value)}>
              <option value="">Not decided yet</option>
              {artists.map((a) => (
                <option key={a.slug} value={a.slug}>
                  {a.name} · {a.genre} · {a.country}
                </option>
              ))}
            </select>
          </label>
        </div>

        <label className="field" htmlFor="finder-message">
          <span className="field__label">
            Message to the venue <span className="muted">(optional, sent with your request)</span>
          </span>
          <textarea id="finder-message" className="field__input finder__message" rows={2} maxLength={1000} placeholder="Gear, set times, guest list…" value={message} onChange={(e) => setMessage(e.target.value)} />
        </label>

        <div className="finder__actions">
          <button type="submit" className="btn btn--primary" disabled={busy}>
            {busy ? "Checking every space…" : "Find open dates"}
          </button>
          {error && (
            <p className="finder__error" role="alert">
              {error}
            </p>
          )}
          {note && <p className="muted small">{note}</p>}
        </div>
      </form>

      {conv.proposals.length > 0 && (
        <div className="slots finder__slots">
          {conv.proposals.map((p) => (
            <SlotCard key={p.id} p={p} tz={tz} city={city} booking={conv.booking} onRequest={requestSlot} />
          ))}
        </div>
      )}
    </section>
  );
}
