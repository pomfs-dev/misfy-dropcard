import { useState } from "react";
import { api, errorMessage } from "../../api/client";
import type { Settlement } from "../../api/types";
import { IconClock } from "../../components/icons";
import { EmptyState, KV, Loadable, SampleBadge, StateChip } from "../../components/ui";
import { useDemo } from "../../lib/demo";
import { fmtDateTime, money, venueTimeLabel } from "../../lib/format";
import { useApi, usePoll } from "../../lib/useApi";
import { useVenue } from "../../lib/venue";

function inFlight(s: Settlement) {
  return s.status === "planned" || s.status === "submitted" || s.items.some((i) => i.status === "pending" || i.status === "planned");
}

function Batch({ s, tz }: { s: Settlement; tz: number }) {
  return (
    <article className={`batch${s.sample ? " batch--sample" : ""}`}>
      <header className="batch__head">
        <div>
          <h3 className="batch__title">{s.booking_title}</h3>
          <p className="batch__id">
            <code>{s.sender_batch_id}</code>
            {s.paypal_batch_id && (
              <>
                {" "}
                · PayPal batch <code>{s.paypal_batch_id}</code>
              </>
            )}
          </p>
        </div>
        <div className="batch__chips">
          <SampleBadge show={s.sample} />
          <StateChip status={s.status} />
        </div>
      </header>
      <KV
        items={[
          ["Due", fmtDateTime(s.scheduled_for, tz)],
          ["Sent to PayPal", s.submitted_at ? fmtDateTime(s.submitted_at, tz) : "—"],
          ["Completed", s.completed_at ? fmtDateTime(s.completed_at, tz) : "—"],
          ["Net paid out", money(s.net_cents)],
          ["Platform fee", money(s.platform_fee_cents)],
        ]}
      />
      {s.last_error && (
        <p className="paypal__error" role="alert">
          {s.last_error}
        </p>
      )}
      <div className="table-scroll">
        <table className="items">
          <thead>
            <tr>
              <th>Recipient</th>
              <th>Email</th>
              <th className="num">Amount</th>
              <th>Status</th>
              <th className="num">Attempts</th>
              <th>PayPal item</th>
            </tr>
          </thead>
          <tbody>
            {s.items.map((it) => (
              <tr key={it.role}>
                <td className="items__role">{it.role}</td>
                <td className="mono">{it.recipient_email}</td>
                <td className="num">{money(it.amount_cents)}</td>
                <td>
                  <StateChip status={it.status} />
                  {it.error && <div className="items__error">{it.error}</div>}
                </td>
                <td className="num">{it.attempts}</td>
                <td className="mono">{it.paypal_item_id ?? "—"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </article>
  );
}

export function PayoutsTab() {
  const { slug, tz, city } = useVenue();
  const { epoch, bump } = useDemo();
  const payouts = useApi(() => api.payouts(slug), [slug, epoch]);
  const [running, setRunning] = useState(false);
  const [runMsg, setRunMsg] = useState<string | null>(null);
  const [runErr, setRunErr] = useState<string | null>(null);
  const [showAllSample, setShowAllSample] = useState(false);

  const mine = (payouts.data?.batches ?? []).filter((b) => !b.sample);
  const watching = mine.some(inFlight);
  usePoll(
    () => api.payouts(slug),
    3000,
    watching,
    (p) => {
      payouts.setData(p);
      return !p.batches.filter((b) => !b.sample).some(inFlight);
    },
  );

  const run = async () => {
    setRunning(true);
    setRunErr(null);
    setRunMsg(null);
    try {
      const before = new Set((payouts.data?.batches ?? []).map((b) => `${b.id}:${b.status}`));
      const res = await api.runSettlement(slug);
      payouts.setData(res);
      const changed = res.batches.filter((b) => !before.has(`${b.id}:${b.status}`)).length;
      setRunMsg(changed ? `Settlement ran: ${changed} batch${changed === 1 ? "" : "es"} updated.` : "Settlement ran: nothing was due yet.");
      bump();
    } catch (e) {
      setRunErr(errorMessage(e));
    } finally {
      setRunning(false);
    }
  };

  return (
    <div className="stack">
      <section className="card payout-intro">
        <div className="payout-intro__text">
          <h2 className="card__title">
            <IconClock size={17} /> Payouts run at 09:00 venue time on the day after the show
          </h2>
          <p className="card__subtitle">
            One PayPal Payouts batch pays the venue (deposit + its ticket share), the artist and the promoter. Fees are
            deducted at settlement. Background jobs run every 15 seconds; “Run settlement now” runs them immediately with
            your demo clock ({venueTimeLabel(tz, city)}).
          </p>
        </div>
        <div className="payout-intro__actions">
          <button type="button" className="btn btn--primary" onClick={run} disabled={running}>
            {running ? "Running…" : "Run settlement now"}
          </button>
          {runMsg && <p className="small muted">{runMsg}</p>}
          {runErr && (
            <p className="paypal__error" role="alert">
              {runErr}
            </p>
          )}
        </div>
      </section>

      <Loadable state={payouts} what="Payouts" endpoint={`GET /api/console/${slug}/payouts`}>
        {(p) => {
          const own = p.batches.filter((b) => !b.sample);
          const sample = p.batches
            .filter((b) => b.sample)
            .sort((a, b) => Date.parse(b.scheduled_for) - Date.parse(a.scheduled_for));
          const shownSample = showAllSample ? sample : sample.slice(0, 2);
          return (
            <>
              <section className="card">
                <header className="card__head">
                  <div>
                    <h2 className="card__title">Your settlement batches</h2>
                    <p className="card__subtitle">Shows from your demo session, sent to the PayPal sandbox.</p>
                  </div>
                  {watching && <span className="live-dot">Live</span>}
                </header>
                {own.length === 0 ? (
                  <EmptyState title="No settlements yet">
                    Book a show, pay the deposit, sell a ticket, then move the demo clock “+1 week” and run settlement.
                  </EmptyState>
                ) : (
                  <div className="batches">
                    {own.map((s) => (
                      <Batch key={s.id} s={s} tz={tz} />
                    ))}
                  </div>
                )}
              </section>

              {sample.length > 0 && (
                <section className="card">
                  <header className="card__head">
                    <div>
                      <h2 className="card__title">Sample history</h2>
                      <p className="card__subtitle">Fictional past shows, settled with the same rule. Never sent to PayPal.</p>
                    </div>
                  </header>
                  <div className="batches">
                    {shownSample.map((s) => (
                      <Batch key={s.id} s={s} tz={tz} />
                    ))}
                  </div>
                  {sample.length > 2 && (
                    <button type="button" className="btn btn--ghost btn--sm" onClick={() => setShowAllSample((v) => !v)}>
                      {showAllSample ? "Show fewer" : `Show all ${sample.length} sample batches`}
                    </button>
                  )}
                </section>
              )}
            </>
          );
        }}
      </Loadable>
    </div>
  );
}
