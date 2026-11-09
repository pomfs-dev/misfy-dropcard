import { useMemo } from "react";
import type { ColDef } from "ag-grid-community";
import type { CustomCellRendererProps } from "ag-grid-react";
import { api } from "../../api/client";
import type { PaymentOrder, WebhookEvent } from "../../api/types";
import { Grid } from "../../components/Grid";
import { IconShield } from "../../components/icons";
import { Chip, Loadable, SampleBadge, StateChip } from "../../components/ui";
import { useDemo } from "../../lib/demo";
import { countryName, fmtStamp, money } from "../../lib/format";
import { useApi, usePoll } from "../../lib/useApi";
import { useVenue } from "../../lib/venue";

function StatusCell(p: CustomCellRendererProps) {
  if (p.node.rowPinned) return null;
  return <StateChip status={p.value as string} />;
}

function SourceCell(p: CustomCellRendererProps<PaymentOrder>) {
  if (p.node.rowPinned || !p.data) return null;
  return p.data.source === "sample" ? <SampleBadge /> : <Chip tone="blue">PayPal sandbox</Chip>;
}

function ConfirmedCell(p: CustomCellRendererProps<PaymentOrder>) {
  if (p.node.rowPinned || !p.data) return null;
  if (p.data.confirmed_by === "webhook") return <Chip tone="green">Verified webhook</Chip>;
  if (p.data.confirmed_by === "reconciliation") return <Chip tone="blue">Reconciliation</Chip>;
  return <span className="muted">—</span>;
}

function SignatureCell(p: CustomCellRendererProps<WebhookEvent>) {
  if (!p.data) return null;
  return p.data.signature_valid ? <Chip tone="green">Valid</Chip> : <Chip tone="red">Invalid</Chip>;
}

const mono = { cellClass: "mono" };

export function PaymentsTab() {
  const { slug, tz } = useVenue();
  const { epoch, config } = useDemo();
  const payments = useApi(() => api.payments(slug), [slug, epoch]);
  usePoll(
    () => api.payments(slug),
    4000,
    payments.data !== null,
    (p) => {
      payments.setData(p);
      return false;
    },
  );

  const orderCols = useMemo<ColDef<PaymentOrder>[]>(
    () => [
      {
        field: "created_at",
        headerName: "Created",
        width: 160,
        sort: "desc",
        valueFormatter: (p) => (p.node?.rowPinned ? "" : p.value ? fmtStamp(p.value, tz) : ""),
      },
      { field: "booking_title", headerName: "Show", flex: 1.4, minWidth: 200 },
      {
        field: "kind",
        headerName: "Kind",
        width: 105,
        valueFormatter: (p) => (p.value ? String(p.value).replace(/^./, (c) => c.toUpperCase()) : ""),
      },
      { field: "quantity", headerName: "Qty", width: 80, type: "rightAligned", filter: "agNumberColumnFilter" },
      {
        field: "amount_cents",
        headerName: "Amount",
        width: 120,
        type: "rightAligned",
        filter: "agNumberColumnFilter",
        valueFormatter: (p) => money(p.value as number),
      },
      {
        field: "paypal_fee_cents",
        headerName: "PayPal fee",
        width: 115,
        type: "rightAligned",
        filter: "agNumberColumnFilter",
        valueFormatter: (p) => (p.value === null || p.value === undefined ? "—" : money(p.value as number)),
      },
      { field: "status", headerName: "Status", width: 120, cellRenderer: StatusCell },
      { field: "confirmed_by", headerName: "Confirmed by", width: 160, cellRenderer: ConfirmedCell },
      {
        field: "payer_country",
        headerName: "Buyer country",
        width: 140,
        valueFormatter: (p) => (p.node?.rowPinned ? "" : countryName(p.value as string | null)),
      },
      { field: "source", headerName: "Source", width: 135, cellRenderer: SourceCell },
      { field: "paypal_order_id", headerName: "PayPal order", width: 170, ...mono, valueFormatter: (p) => p.value ?? "—" },
      { field: "request_id", headerName: "Idempotency key", width: 300, ...mono },
    ],
    [tz],
  );

  const hookCols = useMemo<ColDef<WebhookEvent>[]>(
    () => [
      {
        field: "received_at",
        headerName: "Received",
        width: 170,
        sort: "desc",
        valueFormatter: (p) => (p.value ? fmtStamp(p.value, tz) : ""),
      },
      { field: "event_type", headerName: "Event", flex: 1, minWidth: 230, ...mono },
      { field: "resource_id", headerName: "Resource", width: 200, ...mono, valueFormatter: (p) => p.value ?? "—" },
      { field: "signature_valid", headerName: "Signature", width: 120, cellRenderer: SignatureCell },
      { field: "outcome", headerName: "Outcome", width: 150, cellRenderer: StatusCell },
      { field: "error", headerName: "Error", flex: 1, minWidth: 180, valueFormatter: (p) => p.value ?? "" },
    ],
    [tz],
  );

  return (
    <Loadable state={payments} what="Payments" endpoint={`GET /api/console/${slug}/payments`}>
      {(p) => {
        const captured = p.orders.filter((o) => o.status === "captured");
        const total = {
          booking_title: `Total · ${captured.length} captured`,
          quantity: captured.reduce((n, o) => n + o.quantity, 0),
          amount_cents: captured.reduce((n, o) => n + o.amount_cents, 0),
          paypal_fee_cents: captured.reduce((n, o) => n + (o.paypal_fee_cents ?? 0), 0),
        };
        const live = p.orders.filter((o) => o.source === "paypal_sandbox").length;
        return (
          <div className="stack">
            <section className="card">
              <header className="card__head">
                <div>
                  <h2 className="card__title">Orders</h2>
                  <p className="card__subtitle">
                    PayPal Orders v2, created by the server with a <code>PayPal-Request-Id</code> idempotency key.
                    {live > 0 ? ` ${live} sandbox order${live === 1 ? "" : "s"} from your session.` : ""}
                  </p>
                </div>
              </header>
              <Grid
                rows={p.orders}
                columns={orderCols}
                pinnedBottom={[total]}
                getRowId={(o) => o.id}
                height={440}
                emptyText="No orders yet"
              />
            </section>

            <section className="card">
              <header className="card__head">
                <div>
                  <h2 className="card__title">Webhook events</h2>
                  <p className="card__subtitle">
                    PayPal → <code>POST /api/paypal/webhook</code>. A booking is confirmed only by an event whose
                    signature PayPal verified.
                  </p>
                </div>
                {config?.value && (
                  <Chip tone={config.value.webhook_verification ? "green" : "yellow"}>
                    <IconShield size={13} /> Signature check {config.value.webhook_verification ? "on" : "not configured"}
                  </Chip>
                )}
              </header>
              <Grid
                rows={p.webhooks}
                columns={hookCols}
                getRowId={(w) => String(w.id)}
                height={p.webhooks.length ? 300 : 160}
                emptyText="No webhook events yet. They appear when PayPal calls the server."
              />
            </section>
          </div>
        );
      }}
    </Loadable>
  );
}
