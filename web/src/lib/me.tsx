import { createContext, type ComponentChildren } from "preact";
import { useContext, useEffect, useMemo, useRef, useState } from "preact/hooks";
import { api } from "../api/client";
import type { Me } from "../api/types";
import { useOnForeground } from "./hooks";
import { createMeLoader } from "./me-loader";
import { syncBadge } from "./pwa";

interface MeContextValue {
  me: Me | null;
  error: Error | null;
  /** Coalesced reload; call after anything that may change counts. */
  reload: () => void;
  /** Replaces Me with a newer answer (`PATCH /api/me`); a reload already on its way is dropped. */
  setMe: (me: Me) => void;
}

const MeContext = createContext<MeContextValue>({
  me: null,
  error: null,
  reload: () => {},
  setMe: () => {},
});

export function MeProvider({ children }: { children: ComponentChildren }) {
  const [me, showMe] = useState<Me | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const timer = useRef<number | null>(null);

  const { reload, start, setMe } = useMemo(() => {
    const loader = createMeLoader(() => api.me(), { me: showMe, error: setError });
    const run = () => {
      timer.current = null;
      return loader.load();
    };
    return {
      start: run,
      setMe: loader.set,
      reload: () => {
        if (timer.current === null) timer.current = window.setTimeout(() => void run(), 150);
      },
    };
  }, []);

  useEffect(() => {
    // Join the screen's initial session request immediately; only later changes need
    // debouncing. Waiting 150 ms can miss that request and fetch /api/me twice.
    void start();
    window.addEventListener("focus", reload);
    return () => {
      window.removeEventListener("focus", reload);
      if (timer.current !== null) window.clearTimeout(timer.current);
    };
  }, [reload, start]);
  useOnForeground(reload);

  const updates = me?.counts.updates;
  useEffect(() => {
    if (updates !== undefined) void syncBadge(updates);
  }, [updates, me]);

  const value = useMemo(() => ({ me, error, reload, setMe }), [me, error, reload, setMe]);
  return <MeContext.Provider value={value}>{children}</MeContext.Provider>;
}

export function useMe(): MeContextValue {
  return useContext(MeContext);
}

/** Calls `callback` when the server's update count changes (sources refreshed, an update installed). */
export function useOnUpdateCountChange(callback: () => void) {
  const { me } = useMe();
  const count = me?.counts.updates;
  const seen = useRef(count);
  const ref = useRef(callback);
  ref.current = callback;
  useEffect(() => {
    if (count === seen.current) return;
    seen.current = count;
    ref.current();
  }, [count]);
}
