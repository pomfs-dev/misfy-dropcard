import { Link } from "react-router-dom";
import type { VenueHit } from "../../api/types";
import { IconPin } from "../../components/icons";
import { SampleBadge } from "../../components/ui";
import { moneyShort } from "../../lib/format";

export const ACTIVITY_HELP =
  "Activity 0–100: how many shows MiSFY’s discovery data has on record for the venue, as a percentile across the directory. It shows a busy calendar, not live popularity.";

export const DISCOVERY_HELP =
  "City, genres and shows on record come from MiSFY’s discovery data (pseudonymized and rounded). Venue names, rooms and prices are made up for the demo.";

/** One venue from the AI scout's search: where, what, and how active its calendar has been. */
export function VenueHitCard({ v, rank, onAsk, busy }: { v: VenueHit; rank: number; onAsk?: (v: VenueHit) => void; busy?: boolean }) {
  return (
    <article className="vhit">
      <header className="vhit__head">
        <span className="vhit__rank" aria-label={`Rank ${rank}`}>
          {rank}
        </span>
        <div className="vhit__title">
          <Link className="vhit__name" to={`/v/${v.slug}`}>
            {v.name}
          </Link>
          <p className="vhit__where">
            <IconPin size={13} /> {v.city}, {v.country}
            {v.kind ? ` · ${v.kind}` : ""}
          </p>
        </div>
        <div className="vhit__heat" title={ACTIVITY_HELP}>
          <span className="vhit__heatnum">{v.activity}</span>
          <span className="vhit__heatlabel">activity</span>
        </div>
      </header>
      <div className="vhit__bar" aria-hidden>
        <span style={{ width: `${Math.max(2, Math.min(100, v.activity))}%` }} />
      </div>
      <dl className="vhit__signals">
        <div>
          <dt>Shows on record</dt>
          <dd>{v.shows_on_record === null ? "—" : `≈${v.shows_on_record}`}</dd>
        </div>
        <div>
          <dt>Room</dt>
          <dd>{v.capacity} guests</dd>
        </div>
        <div>
          <dt>From</dt>
          <dd>{moneyShort(v.quote_from_cents)}</dd>
        </div>
      </dl>
      <div className="vhit__tags">
        {v.genres.map((g) => (
          <span key={g} className="chip chip--neutral">
            {g}
          </span>
        ))}
        {v.data_source === "discovery" ? (
          <span className="sample-badge" title={DISCOVERY_HELP}>
            Discovery data
          </span>
        ) : (
          <SampleBadge />
        )}
      </div>
      {onAsk && (
        <footer className="vhit__foot">
          <span className="muted small">a night, venue’s quote</span>
          <button type="button" className="btn btn--ghost btn--sm" onClick={() => onAsk(v)} disabled={busy}>
            Check dates here
          </button>
        </footer>
      )}
    </article>
  );
}
