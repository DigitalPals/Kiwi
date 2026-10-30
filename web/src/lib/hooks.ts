import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { BoundedCache } from "./cache";

// Last successful results, so returning to a screen renders immediately. Bound search
// and detail entries for long-running Home Screen sessions.
const cache = new BoundedCache<string, unknown>(32);

export interface Resource<T> {
  data: T | undefined;
  error: Error | null;
  loading: boolean;
  reload: () => Promise<void>;
  mutate: (update: T | ((previous: T | undefined) => T | undefined)) => void;
}

function toError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}

/** Loads data for `key`, keeping cached or previous data visible while reloading. */
export function useResource<T>(key: string, load: () => Promise<T>): Resource<T> {
  const [state, setState] = useState(() => ({
    data: cache.get(key) as T | undefined,
    error: null as Error | null,
    loading: true,
  }));
  const loadRef = useRef(load);
  loadRef.current = load;
  const seq = useRef(0);

  const reload = useCallback(async () => {
    const id = ++seq.current;
    setState((s) => ({ ...s, loading: true }));
    try {
      const data = await loadRef.current();
      if (id !== seq.current) return;
      cache.set(key, data);
      setState({ data, error: null, loading: false });
    } catch (error) {
      if (id !== seq.current) return;
      setState((s) => ({ ...s, error: toError(error), loading: false }));
    }
  }, [key]);

  useEffect(() => {
    const cached = cache.get(key) as T | undefined;
    if (cached !== undefined) setState((s) => ({ ...s, data: cached }));
    void reload();
    return () => { seq.current++; };
  }, [key, reload]);

  const mutate = useCallback(
    (update: T | ((previous: T | undefined) => T | undefined)) =>
      setState((s) => {
        const data = update instanceof Function ? update(s.data) : update;
        if (data === undefined) return s;
        cache.set(key, data);
        return { ...s, data };
      }),
    [key],
  );

  return { ...state, reload, mutate };
}

/** Runs `callback` when the app returns to the foreground. */
export function useOnForeground(callback: () => void) {
  const ref = useRef(callback);
  ref.current = callback;
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState === "visible") ref.current();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, []);
}

export function useDebounced<T>(value: T, ms: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = window.setTimeout(() => setDebounced(value), ms);
    return () => window.clearTimeout(timer);
  }, [value, ms]);
  return debounced;
}

/** Wraps an async handler with a pending flag and an error message. */
export function useTask<A extends unknown[]>(task: (...args: A) => Promise<unknown>) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef(task);
  ref.current = task;
  const run = useCallback(async (...args: A) => {
    setPending(true);
    setError(null);
    try {
      await ref.current(...args);
      return true;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return false;
    } finally {
      setPending(false);
    }
  }, []);
  return { run, pending, error, setError };
}

export function useStoredFlag(storageKey: string): [boolean, (value: boolean) => void] {
  const [value, setValue] = useState(() => {
    try {
      return localStorage.getItem(storageKey) === "1";
    } catch {
      return false;
    }
  });
  const set = useCallback(
    (next: boolean) => {
      setValue(next);
      try {
        if (next) localStorage.setItem(storageKey, "1");
        else localStorage.removeItem(storageKey);
      } catch {
        // Private mode or storage disabled: remember for this session only.
      }
    },
    [storageKey],
  );
  return [value, set];
}
