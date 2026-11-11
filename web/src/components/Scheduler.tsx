import { useEffect, useRef, useState } from "react";
import { SchedulerPro, StringHelper, type EventModel } from "@bryntum/schedulerpro";
import "@bryntum/schedulerpro/schedulerpro.css";
import "@bryntum/schedulerpro/svalbard-light.css";
import "@bryntum/schedulerpro/fontawesome/css/fontawesome.css";
import "@bryntum/schedulerpro/fontawesome/css/solid.css";
import "../styles/scheduler.css";
import type { BookingPatch, BookingStatus, Schedule, ScheduleEvent, Space } from "../api/types";
import {
  addLocalDays,
  capacity,
  fmtDateLong,
  fmtTimeRange,
  localDayDiff,
  moneyShort,
  shiftIso,
  startOfLocalDay,
  toVenueLocal,
} from "../lib/format";
import { bookingStatusLabel } from "./ui";

export type SchedulerProps = {
  schedule: Schedule;
  /** Owner console: the visitor may drag/resize their own proposals and holds. */
  editable?: boolean;
  /** Bars to flash (e.g. the proposals of the latest chat turn); the first one is scrolled into view. */
  highlightIds?: string[];
  selectedId?: string | null;
  onEventClick?: (ev: ScheduleEvent) => void;
  /** Resolve to keep the change, reject to revert it (e.g. 409 overlap). */
  onMove?: (ev: ScheduleEvent, patch: BookingPatch) => Promise<void>;
  city?: string | null;
  height?: number;
  rowHeight?: number;
};

const EDITABLE: BookingStatus[] = ["ai_proposed", "hold"];

type Display = { start: Date; end: Date };

type ResizeContext = {
  eventRecord?: { id: string | number };
  startDate: Date;
  endDate: Date;
  async: boolean;
  finalize: (ok: boolean) => void;
};

/**
 * A booking blocks its space for the venue day(s) it touches (spaces are rented per day), so bars
 * fill whole day columns; the exact venue-time hours are written inside the bar. Shows ending
 * before 06:00 belong to the previous day.
 */
function displaySpan(ev: ScheduleEvent, tz: number): Display {
  const start = startOfLocalDay(toVenueLocal(ev.starts_at, tz));
  const endLocal = toVenueLocal(Date.parse(ev.ends_at) - 6 * 3600_000, tz);
  let end = addLocalDays(startOfLocalDay(endLocal), 1);
  if (end <= start) end = addLocalDays(start, 1);
  return { start, end };
}

function canEdit(ev: ScheduleEvent, editable: boolean) {
  return editable && ev.mine && EDITABLE.includes(ev.status);
}

function enc(s: string) {
  return StringHelper.encodeHtml(s);
}

export function Scheduler({
  schedule,
  editable = false,
  highlightIds,
  selectedId,
  onEventClick,
  onMove,
  city,
  height = 344,
  rowHeight = 58,
}: SchedulerProps) {
  const host = useRef<HTMLDivElement>(null);
  const sched = useRef<SchedulerPro | null>(null);
  const events = useRef(new Map<string, ScheduleEvent>());
  const spaces = useRef(new Map<string, Space>());
  const displays = useRef(new Map<string, Display>());
  const tzRef = useRef(schedule.tz_offset_minutes);
  const highlight = useRef(new Set<string>());
  const selected = useRef<string | null>(selectedId ?? null);
  const clickRef = useRef(onEventClick);
  const moveRef = useRef(onMove);
  const firstLoad = useRef(true);
  const lastSpan = useRef<string>("");
  const [error, setError] = useState<string | null>(null);
  clickRef.current = onEventClick;
  moveRef.current = onMove;

  // Create once, destroy on unmount.
  useEffect(() => {
    if (!host.current) return;
    const narrow = window.matchMedia("(max-width: 640px)").matches;
    const isEditable = editable;

    const finalizeMove = (
      id: string,
      patch: BookingPatch,
      finalize: (ok: boolean) => void,
    ) => {
      const ev = events.current.get(id);
      const mover = moveRef.current;
      if (!ev || !mover) {
        finalize(false);
        return;
      }
      setError(null);
      mover(ev, patch).then(
        () => finalize(true),
        (e: unknown) => {
          finalize(false);
          setError(e instanceof Error ? e.message : "The change was not saved.");
        },
      );
    };

    const s = new SchedulerPro({
      appendTo: host.current,
      readOnly: !isEditable,
      rowHeight,
      barMargin: 6,
      resourceMargin: 6,
      eventStyle: null,
      eventLayout: "stack",
      zoomOnMouseWheel: false,
      zoomOnTimeAxisDoubleClick: false,
      enableEventAnimations: true,
      useInitialAnimation: false,
      allowOverlap: true,
      emptyText: "No spaces",
      viewPreset: {
        base: "weekAndDay",
        id: "dcDays",
        tickWidth: narrow ? 76 : 104,
        displayDateFormat: "ddd MMM D",
        shiftUnit: "week",
        timeResolution: { unit: "day", increment: 1 },
        headers: [
          { unit: "month", dateFormat: "MMMM YYYY" },
          {
            unit: "day",
            renderer: (startDate: Date, _end: Date, cfg: { headerCellCls?: string }) => {
              const dow = startDate.getDay();
              if (dow === 5 || dow === 6) cfg.headerCellCls = "dc-head-weekend";
              return `${["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][dow]} ${startDate.getDate()}`;
            },
          },
        ],
      },
      columns: [
        {
          text: "Space",
          field: "name",
          width: narrow ? 112 : 176,
          sortable: false,
          editor: false,
          renderer: ({ record }: { record: { id: string | number } }) => {
            const sp = spaces.current.get(String(record.id));
            if (!sp) return "";
            return {
              tag: "div",
              className: "dc-res",
              children: [
                { tag: "div", className: "dc-res__name", text: sp.name },
                {
                  tag: "div",
                  className: "dc-res__meta",
                  text: narrow
                    ? `${capacity(sp.capacity_min, sp.capacity_max)} cap`
                    : `${capacity(sp.capacity_min, sp.capacity_max)} cap · ${moneyShort(sp.day_rate_cents)}/day`,
                },
              ],
            };
          },
        },
      ],
      features: {
        cellEdit: false,
        cellMenu: false,
        eventMenu: false,
        scheduleMenu: false,
        headerMenu: false,
        timeAxisHeaderMenu: false,
        taskEdit: false,
        dependencies: false,
        dependencyEdit: false,
        eventDragCreate: false,
        eventDragSelect: false,
        eventCopyPaste: false,
        nonWorkingTime: false,
        // Bars are our own DOM filling the event element; sticky content would collapse them.
        stickyEvents: false,
        sort: false,
        columnReorder: false,
        stripe: false,
        eventDrag: isEditable ? { constrainDragToTimeline: true, showTooltip: true } : false,
        eventResize: isEditable ? { showTooltip: true } : false,
        timeRanges: { showCurrentTimeLine: false, showHeaderElements: true, enableResizing: false },
        eventTooltip: {
          hoverDelay: 120,
          template: ({ eventRecord }: { eventRecord: { id: string | number } }) => {
            const ev = events.current.get(String(eventRecord.id));
            if (!ev) return "";
            const tz = tzRef.current;
            const sp = spaces.current.get(ev.space_id);
            const who = ev.mine ? "Your booking" : ev.sample ? "Sample" : "";
            return `<div class="dc-tip">
              <div class="dc-tip__title">${enc(ev.title)}</div>
              <div class="dc-tip__row">${enc(bookingStatusLabel(ev.status))}${who ? ` · ${enc(who)}` : ""}</div>
              <div class="dc-tip__row">${enc(sp?.name ?? "")}</div>
              <div class="dc-tip__row">${enc(fmtDateLong(ev.starts_at, tz))}</div>
              <div class="dc-tip__row">${enc(fmtTimeRange(ev.starts_at, ev.ends_at, tz))}${city ? ` · ${enc(city)} time` : ""}</div>
              ${canEdit(ev, isEditable) ? `<div class="dc-tip__hint">Drag to move · resize the edges to change days</div>` : ""}
            </div>`;
          },
        },
      },
      eventRenderer: ({ eventRecord }) => {
        const id = String(eventRecord.id);
        const ev = events.current.get(id);
        if (!ev) return eventRecord.name ?? "";
        const tz = tzRef.current;
        const classes = [
          "dc-bar",
          `dc-bar--${ev.status}`,
          ev.mine ? "dc-bar--mine" : "",
          ev.sample ? "dc-bar--sample" : "",
          highlight.current.has(id) ? "dc-bar--new" : "",
          selected.current === id ? "dc-bar--selected" : "",
          canEdit(ev, isEditable) ? "dc-bar--editable" : "",
        ]
          .filter(Boolean)
          .join(" ");
        const status = ev.status === "ai_proposed" ? "Proposal" : bookingStatusLabel(ev.status);
        const time = fmtTimeRange(ev.starts_at, ev.ends_at, tz).replace(/:00/g, "");
        return {
          tag: "div",
          className: classes,
          children: [
            { tag: "div", className: "dc-bar__title", text: ev.title },
            { tag: "div", className: "dc-bar__meta", text: ev.mine ? `${time} · ${status} · yours` : `${time} · ${status}` },
          ],
        };
      },
      listeners: {
        eventClick: ({ eventRecord }: { eventRecord: { id: string | number } }) => {
          const ev = events.current.get(String(eventRecord.id));
          if (ev) clickRef.current?.(ev);
        },
        beforeEventDrag: ({ eventRecord }: { eventRecord: { id: string | number } }) => {
          const ev = events.current.get(String(eventRecord.id));
          return !!ev && canEdit(ev, isEditable);
        },
        beforeEventResize: ({ eventRecord }: { eventRecord: { id: string | number } }) => {
          const ev = events.current.get(String(eventRecord.id));
          return !!ev && canEdit(ev, isEditable);
        },
        beforeEventDropFinalize: ({ context }) => {
          const rec = context.eventRecord ?? context.eventRecords?.[0];
          if (!rec) return;
          const id = String(rec.id);
          const ev = events.current.get(id);
          const disp = displays.current.get(id);
          if (!ev || !disp || !context.valid) return;
          const newStart = context.startDate as Date;
          const days = localDayDiff(disp.start, newStart);
          const target = (context.newResource ?? context.resourceRecord) as { id: string | number } | undefined;
          const spaceId = target ? String(target.id) : ev.space_id;
          if (days === 0 && spaceId === ev.space_id) return;
          context.async = true;
          const patch: BookingPatch = {};
          if (spaceId !== ev.space_id) patch.space_id = spaceId;
          if (days !== 0) {
            patch.starts_at = shiftIso(ev.starts_at, days);
            patch.ends_at = shiftIso(ev.ends_at, days);
          }
          finalizeMove(id, patch, (ok) => context.finalize(ok));
        },
        // Typed loosely by Bryntum (`context: object`); the documented fields are read below.
        beforeEventResizeFinalize: (event: { context: object; eventRecord?: { id: string | number } }) => {
          const context = event.context as ResizeContext;
          const rec = context.eventRecord ?? event.eventRecord;
          if (!rec) return Promise.resolve();
          const id = String(rec.id);
          const ev = events.current.get(id);
          const disp = displays.current.get(id);
          if (!ev || !disp) return Promise.resolve();
          const startDays = localDayDiff(disp.start, context.startDate);
          const endDays = localDayDiff(disp.end, context.endDate);
          if (startDays === 0 && endDays === 0) return Promise.resolve();
          context.async = true;
          const patch: BookingPatch = {};
          if (startDays !== 0) patch.starts_at = shiftIso(ev.starts_at, startDays);
          if (endDays !== 0) patch.ends_at = shiftIso(ev.ends_at, endDays);
          finalizeMove(id, patch, (ok) => context.finalize(ok));
          return Promise.resolve();
        },
      },
    });
    sched.current = s;
    // Dev-only handle for UI tests (stripped from production builds).
    if (import.meta.env.DEV) (window as unknown as { __dcScheduler?: SchedulerPro }).__dcScheduler = s;
    return () => {
      sched.current = null;
      s.destroy();
    };
    // The instance is created once; data and callbacks flow in through refs and the effect below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editable]);

  // Load data whenever the schedule changes.
  useEffect(() => {
    const s = sched.current;
    if (!s) return;
    const tz = schedule.tz_offset_minutes;
    tzRef.current = tz;
    const evMap = new Map<string, ScheduleEvent>();
    const dispMap = new Map<string, Display>();
    schedule.events.forEach((e) => {
      evMap.set(e.id, e);
      dispMap.set(e.id, displaySpan(e, tz));
    });
    events.current = evMap;
    displays.current = dispMap;
    spaces.current = new Map(schedule.spaces.map((sp) => [sp.id, sp]));
    highlight.current = new Set(highlightIds ?? []);

    const resourcesData = [...schedule.spaces]
      .sort((a, b) => a.sort_order - b.sort_order || a.name.localeCompare(b.name))
      .map((sp) => ({ id: sp.id, name: sp.name }));
    const eventsData = schedule.events.map((e) => {
      const d = dispMap.get(e.id)!;
      const editableEv = canEdit(e, editable);
      return {
        id: e.id,
        name: e.title,
        startDate: d.start,
        endDate: d.end,
        manuallyScheduled: true,
        draggable: editableEv,
        resizable: editableEv,
      };
    });
    const assignmentsData = schedule.events.map((e) => ({ id: `a-${e.id}`, event: e.id, resource: e.space_id }));

    const viewStart = startOfLocalDay(toVenueLocal(schedule.from, tz));
    const viewEnd = addLocalDays(startOfLocalDay(toVenueLocal(schedule.to, tz)), 1);
    const nowLocal = toVenueLocal(schedule.now, tz);

    // Only move the time axis when the window really changed (first load, demo clock moved).
    const spanKey = `${viewStart.getTime()}-${viewEnd.getTime()}`;
    const spanChanged = spanKey !== lastSpan.current;
    lastSpan.current = spanKey;

    let cancelled = false;
    (async () => {
      if (spanChanged) s.setTimeSpan(viewStart, viewEnd);
      await s.project.loadInlineData({ resources: resourcesData, events: eventsData, assignments: assignmentsData });
      if (cancelled || s.isDestroyed) return;
      s.project.timeRangeStore.data = [{ id: "now", name: "Now", startDate: nowLocal, cls: "dc-now" }];
      const firstHighlight = (highlightIds ?? [])
        .map((id) => s.eventStore.getById(id) as EventModel | undefined)
        .find(Boolean);
      if (firstHighlight) {
        await s.scrollEventIntoView(firstHighlight, { block: "center", animate: 400, highlight: false });
      } else if (firstLoad.current || spanChanged) {
        await s.scrollToDate(addLocalDays(startOfLocalDay(nowLocal), -1), { block: "start", animate: !firstLoad.current });
      }
      firstLoad.current = false;
    })().catch(() => {
      /* scheduler destroyed mid-load */
    });

    // Let the "new" pulse play once, then render the bars normally again.
    let timer: ReturnType<typeof setTimeout> | undefined;
    if (highlight.current.size) {
      timer = setTimeout(() => {
        highlight.current = new Set();
        if (!s.isDestroyed) s.refreshRows();
      }, 6000);
    }
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [schedule, highlightIds, editable]);

  // Selection outline (owner side panel).
  useEffect(() => {
    selected.current = selectedId ?? null;
    const s = sched.current;
    if (s && !s.isDestroyed) s.refreshRows();
  }, [selectedId]);

  return (
    <div className="dc-sched-wrap">
      <div ref={host} className="dc-sched" style={{ height }} />
      {error && (
        <div className="dc-sched__error" role="alert">
          <strong>Change reverted.</strong> {error}
          <button type="button" className="btn btn--ghost btn--sm" onClick={() => setError(null)}>
            Dismiss
          </button>
        </div>
      )}
    </div>
  );
}

export function SchedulerLegend({ editable = false }: { editable?: boolean }) {
  return (
    <div className="legend" aria-label="Legend">
      <span className="legend__item">
        <i className="legend__swatch legend__swatch--paid" /> Paid
      </span>
      <span className="legend__item">
        <i className="legend__swatch legend__swatch--approved" /> Approved
      </span>
      <span className="legend__item">
        <i className="legend__swatch legend__swatch--hold" /> On hold
      </span>
      <span className="legend__item">
        <i className="legend__swatch legend__swatch--ai_proposed" /> Proposal
      </span>
      <span className="legend__item">
        <i className="legend__swatch legend__swatch--completed" /> Completed
      </span>
      <span className="legend__item">
        <i className="legend__swatch legend__swatch--mine" /> Yours
      </span>
      {editable && <span className="legend__hint">Drag your own proposals and holds to move them.</span>}
    </div>
  );
}
