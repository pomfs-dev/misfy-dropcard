import { NavLink } from "react-router-dom";
import { useDemo } from "../lib/demo";
import { fmtDate, fmtTime, venueTimeLabel } from "../lib/format";
import { useOptionalVenue } from "../lib/venue";
import { IconClock, IconX } from "./icons";
import { InlineConfirm } from "./ui";

export const DEFAULT_SLUG = "blue-lantern";

function offsetLabel(seconds: number): string | null {
  if (!seconds) return null;
  const totalHours = Math.round(seconds / 3600);
  const d = Math.floor(totalHours / 24);
  const h = totalHours % 24;
  if (d && h) return `+${d}d ${h}h`;
  if (d) return `+${d} day${d === 1 ? "" : "s"}`;
  return `+${h}h`;
}

export function TopBar() {
  const venue = useOptionalVenue();
  const slug = venue?.slug ?? DEFAULT_SLUG;
  const demo = useDemo();
  const tz = venue?.tz ?? 540;
  const city = venue?.city ?? null;
  const offset = demo.session ? offsetLabel(demo.session.clock_offset_seconds) : null;

  return (
    <header className="topbar">
      <div className="topbar__inner">
        <NavLink to="/find" className="wordmark" aria-label="MiSFY DropCard home">
          <img className="wordmark__logo" src="/brand/misfy-logo.png" alt="MiSFY" width={64} height={24} />
          <span className="wordmark__text">DropCard</span>
        </NavLink>

        <nav className="roleswitch" aria-label="Role">
          <NavLink to="/find" className={({ isActive }) => `roleswitch__item${isActive ? " is-active" : ""}`}>
            Find a venue
          </NavLink>
          <NavLink to={`/v/${slug}`} className={({ isActive }) => `roleswitch__item${isActive ? " is-active" : ""}`}>
            Venue card
          </NavLink>
          <NavLink
            to={`/console/${slug}`}
            className={({ isActive }) => `roleswitch__item${isActive ? " is-active" : ""}`}
          >
            Venue owner
          </NavLink>
        </nav>

        <div className="clock" aria-label="Demo clock">
          <div className={`clock__pill${offset ? " is-shifted" : ""}`} title="The demo clock of your session, in venue time">
            <IconClock size={15} />
            {demo.nowMs !== null ? (
              <span className="clock__now">
                <span className="clock__date">{fmtDate(demo.nowMs, tz)}</span>
                <span className="clock__time">{fmtTime(demo.nowMs, tz)}</span>
                <span className="clock__tz">{venueTimeLabel(tz, city)}</span>
              </span>
            ) : (
              <span className="clock__now muted">{demo.sessionError ? "Server unreachable" : "Connecting…"}</span>
            )}
            {offset && <span className="clock__offset">{offset}</span>}
          </div>
          <div className="clock__actions">
            <button
              type="button"
              className="btn btn--ghost btn--sm"
              onClick={() => demo.advance(24)}
              disabled={!demo.session || demo.busy !== null}
            >
              {demo.busy === "advance-day" ? "…" : "+1 day"}
            </button>
            <button
              type="button"
              className="btn btn--ghost btn--sm"
              onClick={() => demo.advance(168)}
              disabled={!demo.session || demo.busy !== null}
            >
              {demo.busy === "advance-week" ? "…" : "+1 week"}
            </button>
            <button
              type="button"
              className="btn btn--ghost btn--sm"
              onClick={() => demo.resetClock()}
              disabled={!demo.session || demo.busy !== null || !demo.session.clock_offset_seconds}
            >
              {demo.busy === "reset-clock" ? "…" : "Reset clock"}
            </button>
            <span className="clock__divider" aria-hidden />
            <InlineConfirm
              label="Reset my demo"
              question="Delete your bookings and chats?"
              confirmLabel="Reset"
              busy={demo.busy === "reset-demo"}
              onConfirm={() => demo.resetDemo()}
            />
          </div>
        </div>
      </div>
      {demo.actionError && (
        <div className="topbar__error" role="alert">
          <span>{demo.actionError}</span>
          <button type="button" className="icon-btn" onClick={demo.clearActionError} aria-label="Dismiss">
            <IconX size={14} />
          </button>
        </div>
      )}
    </header>
  );
}

export function Footer() {
  return (
    <footer className="footer">
      <p>
        Venues, artists and transactions marked Sample are fictional; Discovery data is pseudonymized and rounded. Payments
        run in the PayPal sandbox.
      </p>
    </footer>
  );
}
