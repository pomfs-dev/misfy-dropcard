import { useEffect, useRef } from "react";
import { useSearchParams } from "react-router-dom";
import { api } from "../../api/client";
import { Spinner } from "../../components/ui";
import { useDemo } from "../../lib/demo";
import { usePoll, useApi } from "../../lib/useApi";
import { useVenue } from "../../lib/venue";
import { AgentTab } from "./AgentTab";
import { DashboardTab } from "./DashboardTab";
import { InboxTab } from "./InboxTab";
import { PaymentsTab } from "./PaymentsTab";
import { PayoutsTab } from "./PayoutsTab";
import { ScheduleTab } from "./ScheduleTab";

const TABS = [
  { id: "schedule", label: "Schedule" },
  { id: "inbox", label: "Inbox" },
  { id: "payments", label: "Payments" },
  { id: "payouts", label: "Payouts" },
  { id: "agent", label: "Agent activity" },
  { id: "dashboard", label: "Dashboard" },
] as const;

type TabId = (typeof TABS)[number]["id"];

export function ConsolePage() {
  const { slug, card } = useVenue();
  const { epoch } = useDemo();
  const [params, setParams] = useSearchParams();
  const raw = params.get("tab");
  const tab: TabId = (TABS.find((t) => t.id === raw)?.id ?? "schedule") as TabId;

  // The inbox feeds the tab badge, so it lives here and refreshes in the background.
  const inbox = useApi(() => api.inbox(slug), [slug, epoch]);
  usePoll(
    () => api.inbox(slug),
    5000,
    true,
    (rows) => {
      inbox.setData(rows);
      return false;
    },
  );
  const holds = inbox.data?.filter((b) => b.status === "hold").length ?? 0;

  // On a phone the tab strip scrolls sideways: keep the active tab visible.
  const activeTab = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const el = activeTab.current;
    const strip = el?.parentElement;
    if (el && strip) strip.scrollTo({ left: el.offsetLeft - strip.clientWidth / 2 + el.clientWidth / 2, behavior: "smooth" });
  }, [tab]);

  useEffect(() => {
    if (card.data) document.title = `${card.data.venue.name} console · MiSFY DropCard`;
  }, [card.data]);

  const name = card.data?.venue.name;
  return (
    <div className="console">
      <header className="console__head">
        <div>
          <span className="eyebrow">Venue owner console</span>
          <h1 className="console__title">{name ?? <Spinner />}</h1>
          <p className="console__sub">
            Approve requests, follow PayPal payments and payouts. Owner actions apply to the bookings made in your demo
            session.
          </p>
        </div>
      </header>

      <nav className="tabs" role="tablist" aria-label="Console sections">
        {TABS.map((t) => (
          <button
            key={t.id}
            ref={tab === t.id ? activeTab : undefined}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            className={`tab${tab === t.id ? " is-active" : ""}`}
            onClick={() => setParams(t.id === "schedule" ? {} : { tab: t.id }, { replace: true })}
          >
            {t.label}
            {t.id === "inbox" && holds > 0 && <span className="tab__badge">{holds}</span>}
          </button>
        ))}
      </nav>

      <div className="console__body" role="tabpanel">
        {tab === "schedule" && <ScheduleTab onInboxChange={inbox.reload} />}
        {tab === "inbox" && <InboxTab inbox={inbox} />}
        {tab === "payments" && <PaymentsTab />}
        {tab === "payouts" && <PayoutsTab />}
        {tab === "agent" && <AgentTab />}
        {tab === "dashboard" && <DashboardTab />}
      </div>
    </div>
  );
}
