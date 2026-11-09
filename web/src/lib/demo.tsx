import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, errorMessage } from "../api/client";
import type { AppConfig, Session } from "../api/types";

type Busy = null | "advance-day" | "advance-week" | "reset-clock" | "reset-demo";

export type DemoState = {
  session: Session | null;
  sessionError: string | null;
  /** Ticking estimate of the session's "now" in ms (server now + time elapsed since it was read). */
  nowMs: number | null;
  /** Bumped after every clock change or demo reset: screens refetch what they show. */
  epoch: number;
  /** Bumped only after "Reset my demo" (local conversation state is dropped too). */
  resetEpoch: number;
  busy: Busy;
  actionError: string | null;
  clearActionError: () => void;
  advance: (hours: 24 | 168) => Promise<void>;
  resetClock: () => Promise<void>;
  resetDemo: () => Promise<void>;
  /** Ask every screen to refetch (e.g. after a booking changed). */
  bump: () => void;
  /** null while loading; `available: false` when GET /api/config is not live yet. */
  config: { available: boolean; value: AppConfig | null } | null;
};

const DemoContext = createContext<DemoState | null>(null);

export function DemoProvider({ children }: { children: ReactNode }) {
  const [session, setSession] = useState<Session | null>(null);
  const [sessionError, setSessionError] = useState<string | null>(null);
  const [epoch, setEpoch] = useState(0);
  const [resetEpoch, setResetEpoch] = useState(0);
  const [busy, setBusy] = useState<Busy>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [config, setConfig] = useState<DemoState["config"]>(null);
  const anchor = useRef<{ serverMs: number; perfAt: number } | null>(null);
  const [nowMs, setNowMs] = useState<number | null>(null);
  const [ready, setReady] = useState(false);

  const applySession = useCallback((s: Session) => {
    anchor.current = { serverMs: Date.parse(s.now), perfAt: performance.now() };
    setSession(s);
    setNowMs(Date.parse(s.now));
    setSessionError(null);
  }, []);

  // The first call creates the demo session and sets the dc_sid cookie. Everything else waits for
  // it: parallel first requests without a cookie would each mint a different session.
  useEffect(() => {
    let cancelled = false;
    const loadConfig = () =>
      api
        .config()
        .then((c) => !cancelled && setConfig({ available: true, value: c }))
        // Any failure: PayPal stays disabled, never faked.
        .catch(() => !cancelled && setConfig({ available: false, value: null }));
    const load = (attempt: number) => {
      api
        .session()
        .then((s) => {
          if (cancelled) return;
          applySession(s);
          setReady(true);
          void loadConfig();
        })
        .catch((e) => {
          if (cancelled) return;
          setSessionError(errorMessage(e));
          setReady(true); // render anyway: screens show their own errors
          if (attempt < 20) setTimeout(() => load(attempt + 1), 3000);
        });
    };
    load(0);
    return () => {
      cancelled = true;
    };
  }, [applySession]);

  useEffect(() => {
    const id = setInterval(() => {
      const a = anchor.current;
      if (a) setNowMs(a.serverMs + (performance.now() - a.perfAt));
    }, 10_000);
    return () => clearInterval(id);
  }, []);

  const run = useCallback(
    async (kind: Exclude<Busy, null>, fn: () => Promise<Session>, afterReset = false) => {
      setBusy(kind);
      setActionError(null);
      try {
        const s = await fn();
        applySession(s);
        if (afterReset) setResetEpoch((n) => n + 1);
        setEpoch((n) => n + 1);
      } catch (e) {
        setActionError(errorMessage(e));
      } finally {
        setBusy(null);
      }
    },
    [applySession],
  );

  const value = useMemo<DemoState>(
    () => ({
      session,
      sessionError,
      nowMs,
      epoch,
      resetEpoch,
      busy,
      actionError,
      clearActionError: () => setActionError(null),
      advance: (hours) => run(hours === 24 ? "advance-day" : "advance-week", () => api.advanceClock(hours)),
      resetClock: () => run("reset-clock", api.resetClock),
      resetDemo: () => run("reset-demo", api.resetDemo, true),
      bump: () => setEpoch((n) => n + 1),
      config,
    }),
    [session, sessionError, nowMs, epoch, resetEpoch, busy, actionError, run, config],
  );

  return <DemoContext.Provider value={value}>{ready ? children : <BootScreen />}</DemoContext.Provider>;
}

export function useDemo(): DemoState {
  const ctx = useContext(DemoContext);
  if (!ctx) throw new Error("useDemo outside DemoProvider");
  return ctx;
}

function BootScreen() {
  return (
    <div className="boot" role="status" aria-label="Starting your demo session">
      <span className="spinner" aria-hidden />
    </div>
  );
}
