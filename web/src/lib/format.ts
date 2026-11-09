// Formatting helpers. Every time in the UI is shown in VENUE time (fixed UTC offset from the API,
// e.g. Seoul = +540 minutes), never in the viewer's browser time zone.

const MINUTE = 60_000;
export const DAY_MS = 86_400_000;

/** A Date whose UTC fields equal the venue's wall-clock time (format it with timeZone "UTC"). */
function shifted(iso: string | number | Date, tzMin: number): Date {
  const ms = typeof iso === "string" ? Date.parse(iso) : typeof iso === "number" ? iso : iso.getTime();
  return new Date(ms + tzMin * MINUTE);
}

const fmtCache = new Map<string, Intl.DateTimeFormat>();
function fmt(opts: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  const key = JSON.stringify(opts);
  let f = fmtCache.get(key);
  if (!f) {
    f = new Intl.DateTimeFormat("en-US", { timeZone: "UTC", ...opts });
    fmtCache.set(key, f);
  }
  return f;
}

/** "Fri, Dec 12" */
export function fmtDate(iso: string | number | Date, tzMin: number): string {
  return fmt({ weekday: "short", month: "short", day: "numeric" }).format(shifted(iso, tzMin));
}

/** "Fri, Dec 12, 2026" */
export function fmtDateLong(iso: string | number | Date, tzMin: number): string {
  return fmt({ weekday: "short", month: "short", day: "numeric", year: "numeric" }).format(shifted(iso, tzMin));
}

/** "7:00 PM" */
export function fmtTime(iso: string | number | Date, tzMin: number): string {
  return fmt({ hour: "numeric", minute: "2-digit" }).format(shifted(iso, tzMin));
}

/** "Dec 12, 7:00 PM" */
export function fmtDateTime(iso: string | number | Date, tzMin: number): string {
  return fmt({ month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }).format(shifted(iso, tzMin));
}

/** "Dec 12, 7:04:31 PM" — for logs. */
export function fmtStamp(iso: string | number | Date, tzMin: number): string {
  return fmt({ month: "short", day: "numeric", hour: "numeric", minute: "2-digit", second: "2-digit" }).format(
    shifted(iso, tzMin),
  );
}

function sameVenueDay(a: string, b: string, tzMin: number): boolean {
  const x = shifted(a, tzMin);
  const y = shifted(b, tzMin);
  return (
    x.getUTCFullYear() === y.getUTCFullYear() && x.getUTCMonth() === y.getUTCMonth() && x.getUTCDate() === y.getUTCDate()
  );
}

/** "7:00 PM – 11:00 PM", or with dates when the range crosses midnight. */
export function fmtTimeRange(startIso: string, endIso: string, tzMin: number): string {
  if (sameVenueDay(startIso, endIso, tzMin)) return `${fmtTime(startIso, tzMin)} – ${fmtTime(endIso, tzMin)}`;
  const endsNextMorning = Date.parse(endIso) - Date.parse(startIso) < DAY_MS;
  if (endsNextMorning) return `${fmtTime(startIso, tzMin)} – ${fmtTime(endIso, tzMin)} (+1)`;
  return `${fmtDateTime(startIso, tzMin)} – ${fmtDateTime(endIso, tzMin)}`;
}

/** "UTC+9" */
export function utcOffsetLabel(tzMin: number): string {
  const sign = tzMin >= 0 ? "+" : "−";
  const abs = Math.abs(tzMin);
  const h = Math.floor(abs / 60);
  const m = abs % 60;
  return `UTC${sign}${h}${m ? `:${String(m).padStart(2, "0")}` : ""}`;
}

/** "Seoul time" when the city is known, else "UTC+9". */
export function venueTimeLabel(tzMin: number, city?: string | null): string {
  return city ? `${city} time` : utcOffsetLabel(tzMin);
}

// ---- Bryntum works with browser-local Dates. We hand it Dates whose LOCAL fields equal the venue
// wall clock, so the scheduler draws venue time no matter where the viewer is.

export function toVenueLocal(iso: string | number, tzMin: number): Date {
  const s = shifted(iso, tzMin);
  return new Date(
    s.getUTCFullYear(),
    s.getUTCMonth(),
    s.getUTCDate(),
    s.getUTCHours(),
    s.getUTCMinutes(),
    s.getUTCSeconds(),
    s.getUTCMilliseconds(),
  );
}

export function fromVenueLocal(d: Date, tzMin: number): string {
  const ms =
    Date.UTC(d.getFullYear(), d.getMonth(), d.getDate(), d.getHours(), d.getMinutes(), d.getSeconds(), d.getMilliseconds()) -
    tzMin * MINUTE;
  return new Date(ms).toISOString();
}

export function startOfLocalDay(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

export function addLocalDays(d: Date, days: number): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + days, d.getHours(), d.getMinutes(), d.getSeconds());
}

/** Whole calendar days between two local Dates (DST-safe). */
export function localDayDiff(a: Date, b: Date): number {
  const ua = Date.UTC(a.getFullYear(), a.getMonth(), a.getDate());
  const ub = Date.UTC(b.getFullYear(), b.getMonth(), b.getDate());
  return Math.round((ub - ua) / DAY_MS);
}

export function shiftIso(iso: string, days: number): string {
  // Venue offsets are fixed (no DST), so a venue day is exactly 24h.
  return new Date(Date.parse(iso) + days * DAY_MS).toISOString();
}

// ---- Money (integer cents, USD)

const usd = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
const usd0 = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", maximumFractionDigits: 0 });
const usdCompact = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  notation: "compact",
  maximumFractionDigits: 1,
});

export function money(cents: number | null | undefined): string {
  if (cents === null || cents === undefined || Number.isNaN(cents)) return "—";
  return usd.format(cents / 100);
}

/** "$2,400" (drops cents when they are zero) */
export function moneyShort(cents: number | null | undefined): string {
  if (cents === null || cents === undefined || Number.isNaN(cents)) return "—";
  return cents % 100 === 0 ? usd0.format(cents / 100) : usd.format(cents / 100);
}

/** "$136,211" — whole dollars, for headline numbers. */
export function moneyWhole(cents: number): string {
  return usd0.format(Math.round(cents / 100));
}

export function moneyCompact(cents: number): string {
  return Math.abs(cents) >= 1_000_000 ? usdCompact.format(cents / 100) : moneyShort(cents);
}

export function percent(share: number): string {
  return `${(share * 100).toFixed(share < 0.1 ? 1 : 0)}%`;
}

export function capacity(min: number, max: number): string {
  return min === max ? `${max}` : `${min}–${max}`;
}

const regionNames = (() => {
  try {
    return new Intl.DisplayNames(["en"], { type: "region" });
  } catch {
    return null;
  }
})();

export function countryName(code: string | null | undefined): string {
  if (!code) return "Unknown";
  try {
    return regionNames?.of(code.toUpperCase()) ?? code;
  } catch {
    return code;
  }
}

export function shortId(id: string | null | undefined, n = 8): string {
  if (!id) return "—";
  return id.length > n + 1 ? `${id.slice(0, n)}…` : id;
}
