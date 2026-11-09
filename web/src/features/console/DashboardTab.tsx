import { useMemo, useState } from "react";
import type { ColDef } from "ag-grid-community";
import type { CustomCellRendererProps } from "ag-grid-react";
import type { AgChartOptions } from "ag-charts-community";
import { api } from "../../api/client";
import type { BookingStatus, Dashboard, DashboardShow } from "../../api/types";
import { Chart, viz } from "../../components/Chart";
import { Grid } from "../../components/Grid";
import { Loadable, SampleBadge, StateChip, StatusChip } from "../../components/ui";
import { useDemo } from "../../lib/demo";
import { countryName, fmtDate, money, moneyWhole, percent, toVenueLocal } from "../../lib/format";
import { useApi } from "../../lib/useApi";
import { useVenue } from "../../lib/venue";

function Kpi({ label, value, sub, accent }: { label: string; value: string; sub?: string; accent?: boolean }) {
  return (
    <div className={`kpi${accent ? " kpi--accent" : ""}`}>
      <span className="kpi__label">{label}</span>
      <span className="kpi__value">{value}</span>
      {sub && <span className="kpi__sub">{sub}</span>}
    </div>
  );
}

function ShowCell(p: CustomCellRendererProps<DashboardShow>) {
  if (!p.data || p.node.rowPinned) return <strong>{p.value as string}</strong>;
  return (
    <span className="cell-title">
      <span className="cell-title__text">{p.data.title}</span>
      <SampleBadge show={p.data.sample} />
    </span>
  );
}

function StatusCell(p: CustomCellRendererProps<DashboardShow>) {
  if (!p.data || p.node.rowPinned) return null;
  return <StatusChip status={p.value as BookingStatus} />;
}

function PayoutCell(p: CustomCellRendererProps<DashboardShow>) {
  if (!p.data || p.node.rowPinned) return null;
  return <StateChip status={p.value as string | null} />;
}

const moneyCol = (field: keyof DashboardShow, headerName: string, width = 120): ColDef<DashboardShow> => ({
  field,
  headerName,
  width,
  type: "rightAligned",
  filter: "agNumberColumnFilter",
  valueFormatter: (p) => money(p.value as number),
});

function DashboardView({ d, tz, homeCountry }: { d: Dashboard; tz: number; homeCountry: string }) {
  const [narrow] = useState(() => window.matchMedia("(max-width: 640px)").matches);
  const shows = useMemo(() => [...d.shows].sort((a, b) => Date.parse(a.date) - Date.parse(b.date)), [d.shows]);
  const ticketsTotal = shows.reduce((n, s) => n + s.tickets_sold, 0);
  const t = d.totals;

  // Revenue per show (bars). Category = booking id so two shows on one day stay separate.
  const revenue = useMemo<AgChartOptions>(() => {
    const label = new Map(shows.map((s) => [s.booking_id, fmtDate(s.date, tz).replace(/^\w+, /, "")]));
    return {
      data: shows.map((s) => ({ id: s.booking_id, title: s.title, gross: s.gross_cents / 100, status: s.status, sample: s.sample })),
      series: [
        {
          type: "bar",
          xKey: "id",
          yKey: "gross",
          yName: "Gross sales",
          cornerRadius: 4,
          itemStyler: ({ datum }: { datum: { status: BookingStatus } }) => ({
            fill: datum.status === "completed" ? viz.ink : viz.green,
          }),
          tooltip: {
            renderer: ({ datum }: { datum: { title: string; gross: number; status: BookingStatus; id: string } }) => ({
              heading: label.get(datum.id) ?? "",
              title: datum.title,
              data: [
                { label: "Gross sales", value: money(Math.round(datum.gross * 100)) },
                { label: "Status", value: datum.status === "completed" ? "Settled" : "On sale (deposit paid)" },
              ],
            }),
          },
        },
      ],
      axes: {
        x: { type: "category", position: "bottom", label: { formatter: ({ value }: { value: string }) => label.get(value) ?? "" } },
        y: {
          type: "number",
          position: "left",
          label: { formatter: ({ value }: { value: number }) => (value >= 1000 ? `$${Math.round(value / 1000)}k` : `$${value}`) },
        },
      },
      legend: { enabled: false },
    } as AgChartOptions;
  }, [shows, tz]);

  // Buyer countries (donut): color answers one question, home vs international; names are direct labels.
  const countries = useMemo(() => {
    const sorted = [...d.by_country].sort((a, b) => b.tickets - a.tickets);
    const top = sorted.slice(0, 5);
    const rest = sorted.slice(5);
    const rows = top.map((c) => ({ key: c.country, name: countryName(c.country), tickets: c.tickets, gross: c.gross_cents }));
    if (rest.length) {
      rows.push({
        key: "OTHER",
        name: "Other",
        tickets: rest.reduce((n, c) => n + c.tickets, 0),
        gross: rest.reduce((n, c) => n + c.gross_cents, 0),
      });
    }
    const total = rows.reduce((n, r) => n + r.tickets, 0) || 1;
    return rows.map((r) => ({ ...r, share: r.tickets / total, label: `${r.name} ${Math.round((r.tickets / total) * 100)}%` }));
  }, [d.by_country]);

  const donut = useMemo<AgChartOptions>(
    () =>
      ({
        data: countries,
        series: [
          {
            type: "donut",
            angleKey: "tickets",
            calloutLabelKey: "label",
            legendItemKey: "name",
            innerRadiusRatio: 0.62,
            strokeWidth: 2,
            strokes: [viz.surface],
            fills: [viz.ink],
            itemStyler: ({ datum }: { datum: { key: string } }) => ({
              fill: datum.key === homeCountry ? viz.green : viz.ink,
            }),
            calloutLabel: { enabled: !narrow, fontSize: 12, color: viz.text },
            calloutLine: { colors: ["#A3A3A3"], length: 12 },
            innerLabels: [
              { text: percent(t.international_share), fontSize: 26, fontWeight: 700, color: viz.text },
              { text: "international", fontSize: 12, color: viz.textMuted },
            ],
            tooltip: {
              renderer: ({ datum }: { datum: { name: string; tickets: number; gross: number; share: number } }) => ({
                heading: datum.name,
                data: [
                  { label: "Tickets", value: datum.tickets.toLocaleString() },
                  { label: "Share", value: percent(datum.share) },
                  { label: "Gross", value: money(datum.gross) },
                ],
              }),
            },
          },
        ],
        legend: { enabled: false },
      }) as AgChartOptions,
    [countries, homeCountry, t.international_share, narrow],
  );

  // Cumulative payouts over show dates (settled shows only).
  const payouts = useMemo<AgChartOptions>(() => {
    let sum = 0;
    const data = shows
      .filter((s) => s.payout_status === "succeeded")
      .map((s) => {
        sum += s.venue_cents + s.artist_cents + s.promoter_cents;
        return { date: toVenueLocal(s.date, tz), title: s.title, cum: sum / 100 };
      });
    return {
      data,
      series: [
        {
          type: "area",
          xKey: "date",
          yKey: "cum",
          yName: "Paid out",
          fill: viz.green,
          fillOpacity: 0.22,
          stroke: viz.ink,
          strokeWidth: 2,
          marker: { enabled: true, size: 8, fill: viz.ink, stroke: viz.surface, strokeWidth: 2 },
          tooltip: {
            renderer: ({ datum }: { datum: { date: Date; title: string; cum: number } }) => ({
              heading: datum.date.toLocaleDateString("en-US", { month: "short", day: "numeric" }),
              title: datum.title,
              data: [{ label: "Paid out so far", value: money(Math.round(datum.cum * 100)) }],
            }),
          },
        },
      ],
      axes: {
        x: { type: "time", position: "bottom", label: { format: "%b %e" } },
        y: {
          type: "number",
          position: "left",
          label: { formatter: ({ value }: { value: number }) => (value >= 1000 ? `$${Math.round(value / 1000)}k` : `$${value}`) },
        },
      },
      legend: { enabled: false },
    } as AgChartOptions;
  }, [shows, tz]);

  const columns = useMemo<ColDef<DashboardShow>[]>(
    () => [
      {
        field: "date",
        headerName: "Date",
        width: 130,
        sort: "desc",
        valueFormatter: (p) => (p.node?.rowPinned ? "" : p.value ? fmtDate(p.value as string, tz) : ""),
      },
      { field: "title", headerName: "Show", flex: 1.3, minWidth: 230, cellRenderer: ShowCell },
      { field: "space", headerName: "Space", width: 130 },
      { field: "status", headerName: "Status", width: 125, cellRenderer: StatusCell },
      { field: "tickets_sold", headerName: "Tickets", width: 105, type: "rightAligned", filter: "agNumberColumnFilter" },
      moneyCol("gross_cents", "Gross", 125),
      moneyCol("paypal_fees_cents", "PayPal fees", 125),
      moneyCol("refunds_cents", "Refunds", 110),
      moneyCol("platform_fee_cents", "Platform fee", 125),
      moneyCol("venue_cents", "Venue", 120),
      moneyCol("artist_cents", "Artist", 120),
      moneyCol("promoter_cents", "Promoter", 120),
      { field: "payout_status", headerName: "Payout", width: 125, cellRenderer: PayoutCell },
    ],
    [tz],
  );

  const sum = (k: keyof DashboardShow) => shows.reduce((n, s) => n + (s[k] as number), 0);
  const totalRow = {
    title: `Total · ${shows.length} shows`,
    tickets_sold: ticketsTotal,
    gross_cents: sum("gross_cents"),
    paypal_fees_cents: sum("paypal_fees_cents"),
    refunds_cents: sum("refunds_cents"),
    platform_fee_cents: sum("platform_fee_cents"),
    venue_cents: sum("venue_cents"),
    artist_cents: sum("artist_cents"),
    promoter_cents: sum("promoter_cents"),
  };
  const sampleOnly = shows.length > 0 && shows.every((s) => s.sample);

  return (
    <div className="stack">
      <div className="kpis">
        <Kpi label="Gross sales" value={moneyWhole(t.gross_cents)} sub={`${shows.length} shows · ${ticketsTotal.toLocaleString()} tickets`} />
        <Kpi
          label="PayPal fees"
          value={moneyWhole(t.paypal_fees_cents)}
          sub={t.gross_cents ? `${percent(t.paypal_fees_cents / t.gross_cents)} of gross` : undefined}
        />
        <Kpi label="Platform fee" value={moneyWhole(t.platform_fee_cents)} sub="Deducted at settlement" />
        <Kpi label="Paid out" value={moneyWhole(t.paid_out_cents)} sub="Venue, artists and promoters" accent />
        <Kpi label="International share" value={percent(t.international_share)} sub="Ticket revenue from abroad" />
      </div>
      {sampleOnly && (
        <p className="muted small">
          <SampleBadge /> Every show here is fictional sample data until your own demo bookings settle.
        </p>
      )}

      <div className="charts">
        <section className="card chart-card chart-card--wide">
          <header className="card__head">
            <div>
              <h2 className="card__title">Revenue per show</h2>
              <p className="card__subtitle">Captured PayPal payments per show (deposit and tickets).</p>
            </div>
            <div className="legend">
              <span className="legend__item">
                <i className="legend__swatch legend__swatch--ink" /> Settled
              </span>
              <span className="legend__item">
                <i className="legend__swatch legend__swatch--paid" /> On sale
              </span>
            </div>
          </header>
          <Chart options={revenue} height={260} label="Bar chart of gross sales per show" />
        </section>

        <section className="card chart-card">
          <header className="card__head">
            <div>
              <h2 className="card__title">Buyer countries</h2>
              <p className="card__subtitle">Tickets by buyer country.</p>
            </div>
            <div className="legend">
              <span className="legend__item">
                <i className="legend__swatch legend__swatch--paid" /> {countryName(homeCountry)} (home)
              </span>
              <span className="legend__item">
                <i className="legend__swatch legend__swatch--ink" /> International
              </span>
            </div>
          </header>
          <Chart options={donut} height={narrow ? 220 : 260} label="Donut chart of tickets by buyer country" />
          <ul className="countries">
            {countries.map((c) => (
              <li key={c.key}>
                <i className={`legend__swatch ${c.key === homeCountry ? "legend__swatch--paid" : "legend__swatch--ink"}`} />
                <span className="countries__name">{c.name}</span>
                <span className="countries__share">{percent(c.share)}</span>
                <span className="countries__tickets">{c.tickets.toLocaleString()} tickets</span>
              </li>
            ))}
          </ul>
        </section>

        <section className="card chart-card chart-card--wide">
          <header className="card__head">
            <div>
              <h2 className="card__title">Cumulative payouts</h2>
              <p className="card__subtitle">Money sent with PayPal Payouts, by show date.</p>
            </div>
          </header>
          <Chart options={payouts} height={240} label="Area chart of cumulative payouts over time" />
        </section>
      </div>

      <section className="card">
        <header className="card__head">
          <div>
            <h2 className="card__title">Shows</h2>
            <p className="card__subtitle">Sort and filter any column. Money in USD; the total row sums every show.</p>
          </div>
        </header>
        <Grid rows={shows} columns={columns} pinnedBottom={[totalRow]} getRowId={(s) => s.booking_id} height={460} />
      </section>
    </div>
  );
}

export function DashboardTab() {
  const { slug, tz, card } = useVenue();
  const { epoch } = useDemo();
  const dash = useApi(() => api.dashboard(slug), [slug, epoch]);
  const home = card.data?.venue.country ?? "KR";
  return (
    <Loadable state={dash} what="The dashboard" endpoint={`GET /api/console/${slug}/dashboard`}>
      {(d) => <DashboardView d={d} tz={tz} homeCountry={home} />}
    </Loadable>
  );
}
