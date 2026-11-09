import { useState, type ReactNode } from "react";
import { ApiError } from "../api/client";
import type { BookingStatus } from "../api/types";
import { IconWrench } from "./icons";

// ---- Status chips

type Tone = "neutral" | "green" | "blue" | "yellow" | "red" | "dashed";

const BOOKING_STATUS: Record<BookingStatus, { label: string; tone: Tone }> = {
  ai_proposed: { label: "AI proposed", tone: "dashed" },
  hold: { label: "On hold", tone: "yellow" },
  approved: { label: "Approved", tone: "blue" },
  paid: { label: "Paid", tone: "green" },
  completed: { label: "Completed", tone: "neutral" },
  cancelled: { label: "Cancelled", tone: "red" },
};

export function bookingStatusLabel(s: BookingStatus): string {
  return BOOKING_STATUS[s]?.label ?? s;
}

/** Tones for the free-form statuses of orders, payouts and webhooks. */
export function toneFor(status: string | null | undefined): Tone {
  switch ((status ?? "").toLowerCase()) {
    case "paid":
    case "captured":
    case "succeeded":
    case "success":
    case "completed_payout":
    case "processed":
    case "ok":
      return "green";
    case "approved":
    case "submitted":
    case "created":
      return "blue";
    case "hold":
    case "pending":
    case "pending_webhook":
    case "partial":
    case "unclaimed":
    case "planned":
      return "yellow";
    case "failed":
    case "error":
    case "returned":
    case "declined":
    case "cancelled":
    case "invalid":
      return "red";
    default:
      return "neutral";
  }
}

export function Chip({ tone = "neutral", children, title }: { tone?: Tone; children: ReactNode; title?: string }) {
  return (
    <span className={`chip chip--${tone}`} title={title}>
      {children}
    </span>
  );
}

export function StatusChip({ status }: { status: BookingStatus }) {
  const s = BOOKING_STATUS[status] ?? { label: status, tone: "neutral" as Tone };
  return <Chip tone={s.tone}>{s.label}</Chip>;
}

export function StateChip({ status }: { status: string | null | undefined }) {
  if (!status) return <span className="muted">—</span>;
  return <Chip tone={toneFor(status)}>{status.replace(/_/g, " ")}</Chip>;
}

export function SampleBadge({ show = true }: { show?: boolean }) {
  if (!show) return null;
  return (
    <span className="sample-badge" title="Fictional sample data">
      Sample
    </span>
  );
}

// ---- Layout bits

export function Section({
  title,
  subtitle,
  actions,
  children,
  className = "",
}: {
  title?: ReactNode;
  subtitle?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={`card ${className}`}>
      {(title || actions) && (
        <header className="card__head">
          <div>
            {title && <h2 className="card__title">{title}</h2>}
            {subtitle && <p className="card__subtitle">{subtitle}</p>}
          </div>
          {actions && <div className="card__actions">{actions}</div>}
        </header>
      )}
      {children}
    </section>
  );
}

export function Spinner({ label }: { label?: string }) {
  return (
    <span className="spinner-wrap" role="status">
      <span className="spinner" aria-hidden />
      {label && <span>{label}</span>}
    </span>
  );
}

export function EmptyState({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="empty">
      <p className="empty__title">{title}</p>
      {children && <div className="empty__body">{children}</div>}
    </div>
  );
}

/** Shown when an endpoint the contract marks as "being built" answers 404. Never fakes success. */
export function NotLive({ what, endpoint }: { what: string; endpoint: string }) {
  return (
    <div className="notlive">
      <span className="notlive__icon">
        <IconWrench size={18} />
      </span>
      <div>
        <p className="notlive__title">{what} is not live on this server yet</p>
        <p className="notlive__body">
          It will show up here as soon as the server provides <code>{endpoint}</code>.
        </p>
      </div>
    </div>
  );
}

export function ErrorNote({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  const msg = error instanceof Error ? error.message : String(error);
  return (
    <div className="error-note" role="alert">
      <span>{msg}</span>
      {onRetry && (
        <button type="button" className="btn btn--ghost btn--sm" onClick={onRetry}>
          Try again
        </button>
      )}
    </div>
  );
}

/** Loading / not-live / error / ready switch for one API call. */
export function Loadable<T>({
  state,
  what,
  endpoint,
  children,
  loadingLabel = "Loading…",
}: {
  state: { data: T | null; error: unknown; loading: boolean; reload: () => void };
  what: string;
  endpoint: string;
  children: (data: T) => ReactNode;
  loadingLabel?: string;
}) {
  if (state.data !== null && state.data !== undefined) return <>{children(state.data)}</>;
  if (state.loading) {
    return (
      <div className="loading-block">
        <Spinner label={loadingLabel} />
      </div>
    );
  }
  if (state.error instanceof ApiError && state.error.notFound) return <NotLive what={what} endpoint={endpoint} />;
  if (state.error) return <ErrorNote error={state.error} onRetry={state.reload} />;
  return null;
}

/** A button that asks for an inline confirmation instead of a browser dialog. */
export function InlineConfirm({
  label,
  question,
  confirmLabel,
  onConfirm,
  busy,
  className = "btn btn--ghost btn--sm",
  danger = true,
}: {
  label: ReactNode;
  question: string;
  confirmLabel: string;
  onConfirm: () => void | Promise<void>;
  busy?: boolean;
  className?: string;
  danger?: boolean;
}) {
  const [asking, setAsking] = useState(false);
  if (!asking) {
    return (
      <button type="button" className={className} onClick={() => setAsking(true)} disabled={busy}>
        {label}
      </button>
    );
  }
  return (
    <span className="confirm" role="group" aria-label={question}>
      <span className="confirm__q">{question}</span>
      <button
        type="button"
        className={`btn btn--sm ${danger ? "btn--danger" : "btn--primary"}`}
        disabled={busy}
        onClick={async () => {
          await onConfirm();
          setAsking(false);
        }}
      >
        {busy ? "Working…" : confirmLabel}
      </button>
      <button type="button" className="btn btn--ghost btn--sm" onClick={() => setAsking(false)} disabled={busy}>
        Cancel
      </button>
    </span>
  );
}

export function KV({ items }: { items: [ReactNode, ReactNode][] }) {
  return (
    <dl className="kv">
      {items.map(([k, v], i) => (
        <div className="kv__row" key={i}>
          <dt>{k}</dt>
          <dd>{v}</dd>
        </div>
      ))}
    </dl>
  );
}
