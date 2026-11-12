import type { Card } from "../../api/types";
import { IconCalendar, IconPin, IconUsers } from "../../components/icons";
import { SampleBadge } from "../../components/ui";
import { capacity, countryName, fmtDate, fmtTime, moneyShort } from "../../lib/format";
import { venuePhoto } from "../../lib/photos";

export function VenueHero({ card }: { card: Card }) {
  const v = card.venue;
  return (
    <section className="hero hero--photo" aria-label={v.name} style={{ backgroundImage: `linear-gradient(180deg, rgba(0,0,0,.25) 0%, rgba(0,0,0,.72) 75%), url(${venuePhoto(v.slug)})` }}>
      <div className="hero__top">
        <span className="hero__eyebrow">Live venue · DropCard</span>
        <SampleBadge />
      </div>
      <h1 className="hero__name">{v.name}</h1>
      <p className="hero__tagline">{v.tagline}</p>
      <div className="hero__facts">
        <span className="hero__fact">
          <IconPin size={15} /> {v.city}, {countryName(v.country)}
        </span>
        <span className="hero__fact">
          <IconUsers size={15} /> {v.capacity_band} capacity
        </span>
      </div>
      <div className="hero__tags">
        {v.tags.map((t) => (
          <span className="tag" key={t}>
            {t}
          </span>
        ))}
      </div>
    </section>
  );
}

export function VenueDetails({ card, tz }: { card: Card; tz: number }) {
  const spaces = [...card.spaces].sort((a, b) => a.sort_order - b.sort_order);
  return (
    <section className="card venue-details">
      <p className="venue-details__desc">{card.venue.description}</p>

      <h3 className="subhead">Spaces</h3>
      <ul className="spaces">
        {spaces.map((s) => (
          <li className="space" key={s.id}>
            <div>
              <div className="space__name">{s.name}</div>
              <div className="space__meta">
                <IconUsers size={13} /> {capacity(s.capacity_min, s.capacity_max)} guests
              </div>
            </div>
            <div className="space__rate">
              {moneyShort(s.day_rate_cents)}
              <span>/day</span>
            </div>
          </li>
        ))}
      </ul>

      <h3 className="subhead">Upcoming shows</h3>
      {card.upcoming.length === 0 ? (
        <p className="muted small">No confirmed shows yet.</p>
      ) : (
        <ul className="shows">
          {card.upcoming.map((u) => (
            <li className="show" key={u.id}>
              <div className="show__date">
                <IconCalendar size={14} />
                <span>{fmtDate(u.starts_at, tz)}</span>
              </div>
              <div className="show__body">
                <div className="show__title">{u.title}</div>
                <div className="show__meta">
                  {u.space} · {fmtTime(u.starts_at, tz)}
                </div>
              </div>
              <div className="show__price">{u.ticket_price_cents !== null ? moneyShort(u.ticket_price_cents) : ""}</div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
