import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "../api/client";

export type ApiState<T> = {
  data: T | null;
  error: ApiError | Error | null;
  /** true until the first answer (success or error) arrives */
  loading: boolean;
  /** true while any request is in flight (also during background refetches) */
  refreshing: boolean;
  reload: () => void;
  setData: (next: T | null) => void;
};

/**
 * Fetches on mount and whenever `deps` change. Keeps the previous data while refetching so the
 * screen does not flash. Ignores answers that arrive after a newer request started.
 */
export function useApi<T>(fn: () => Promise<T>, deps: readonly unknown[], enabled = true): ApiState<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<ApiError | Error | null>(null);
  const [loading, setLoading] = useState<boolean>(enabled);
  const [refreshing, setRefreshing] = useState<boolean>(false);
  const [tick, setTick] = useState(0);
  const seq = useRef(0);
  const fnRef = useRef(fn);
  fnRef.current = fn;

  useEffect(() => {
    if (!enabled) {
      setLoading(false);
      return;
    }
    const my = ++seq.current;
    setRefreshing(true);
    fnRef
      .current()
      .then((d) => {
        if (my !== seq.current) return;
        setData(d);
        setError(null);
      })
      .catch((e: unknown) => {
        if (my !== seq.current) return;
        setError(e instanceof Error ? e : new Error(String(e)));
      })
      .finally(() => {
        if (my !== seq.current) return;
        setLoading(false);
        setRefreshing(false);
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick, enabled]);

  const reload = useCallback(() => setTick((t) => t + 1), []);
  return { data, error, loading, refreshing, reload, setData };
}

/** Polls `fn` every `ms` while `active` is true. Stops when `until(result)` returns true. */
export function usePoll<T>(
  fn: () => Promise<T>,
  ms: number,
  active: boolean,
  onResult: (r: T) => boolean,
  onError?: (e: unknown) => void,
): void {
  const fnRef = useRef(fn);
  const resRef = useRef(onResult);
  const errRef = useRef(onError);
  fnRef.current = fn;
  resRef.current = onResult;
  errRef.current = onError;

  useEffect(() => {
    if (!active) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const run = async () => {
      try {
        const r = await fnRef.current();
        if (stopped) return;
        if (resRef.current(r)) return;
      } catch (e) {
        if (stopped) return;
        errRef.current?.(e);
      }
      if (!stopped) timer = setTimeout(run, ms);
    };
    timer = setTimeout(run, ms);
    return () => {
      stopped = true;
      if (timer) clearTimeout(timer);
    };
  }, [active, ms]);
}
