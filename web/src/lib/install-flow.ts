import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { ApiError, api, errorMessage } from "../api/client";
import type { AppState, AppSummary, Build, PrepareRequest } from "../api/types";
import { IDLE, isActiveBuild, isInstallPending, type LocalFlow } from "./action";
import { useMe } from "./me";

const BUILD_POLL_MS = 1000;
const INSTALL_POLL_MS = 2000;
const INSTALL_WATCH_MS = 5 * 60_000;
/** After the watch window, while the server still reports an install (it gives up after 10 minutes). */
const INSTALL_SLOW_POLL_MS = 15_000;

interface Options {
  app: AppSummary;
  onState: (state: AppState) => void;
  /**
   * "app": the main button; a running server build is adopted and the app state is the source
   * of truth once a build finishes. "version": a specific release; keeps its own build because
   * the app's install_url points at the target release.
   */
  mode: "app" | "version";
  prepareBody?: PrepareRequest;
  /** When the install being watched has finished. */
  isDone?: (state: AppState) => boolean;
}

export function useInstallFlow({ app, onState, mode, prepareBody, isDone }: Options) {
  const [local, setLocal] = useState<LocalFlow>(IDLE);
  const { reload: reloadMe } = useMe();
  const latest = useRef({ app, onState, isDone });
  latest.current = { app, onState, isDone };

  const refresh = useCallback(async (): Promise<AppState | null> => {
    const { app: current } = latest.current;
    try {
      const detail = await api.app(current.source_id, current.bundle_id);
      latest.current.onState(detail.state);
      return detail.state;
    } catch {
      return null;
    }
  }, []);

  /** Settles a finished build: in app mode the refreshed app state takes over. */
  const settle = useCallback(
    async (build: Build) => {
      const state = await refresh();
      const keep = mode === "version" || !state || state.action === "preparing";
      setLocal((l) => ({ ...l, busy: false, build: keep ? build : null }));
    },
    [mode, refresh],
  );

  // Counts (updates, preparing, installed) change whenever an app's action does.
  const action = app.state.action;
  const previousAction = useRef(action);
  useEffect(() => {
    if (action === previousAction.current) return;
    previousAction.current = action;
    reloadMe();
  }, [action, reloadMe]);

  // Poll our own running build, or adopt a running server build (e.g. after "Update All").
  const serverBuild = mode === "app" && action === "preparing" ? app.state.build : null;
  const localActive = local.build && isActiveBuild(local.build) ? local.build.id : null;
  const serverActive =
    serverBuild && isActiveBuild(serverBuild) && serverBuild.id !== local.build?.id ? serverBuild.id : null;
  const pollBuildId = localActive ?? serverActive;

  useEffect(() => {
    if (!pollBuildId) return;
    let cancelled = false;
    let timer = 0;
    const tick = async () => {
      try {
        const build = await api.build(pollBuildId);
        if (cancelled) return;
        if (isActiveBuild(build)) setLocal((l) => ({ ...l, build }));
        else {
          await settle(build);
          return;
        }
      } catch (error) {
        if (cancelled) return;
        if (error instanceof ApiError && error.status === 404) {
          setLocal((l) => ({ ...l, build: null, error: "The build was removed. Try again." }));
          return;
        }
        // Transient failure: keep polling.
      }
      timer = window.setTimeout(tick, BUILD_POLL_MS);
    };
    timer = window.setTimeout(tick, BUILD_POLL_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [pollBuildId, settle]);

  const watching = local.watching || (mode === "app" && action === "installing");
  useEffect(() => {
    if (!watching) return;
    const started = Date.now();
    let cancelled = false;
    let timer = 0;
    const done = (state: AppState) => latest.current.isDone?.(state) ?? !isInstallPending(state);
    const tick = async () => {
      const state = await refresh();
      if (cancelled) return;
      if (state && done(state)) {
        setLocal((l) => (l.watching ? { ...l, watching: false } : l));
        return;
      }
      if (Date.now() - started > INSTALL_WATCH_MS) {
        setLocal((l) => (l.watching ? { ...l, watching: false } : l));
        // `watching` stays true (and this effect alive) while the server reports an install, so
        // keep checking slowly until it finishes or the server gives up; otherwise stop here.
        if (mode === "app" && (!state || state.action === "installing")) timer = window.setTimeout(tick, INSTALL_SLOW_POLL_MS);
        return;
      }
      timer = window.setTimeout(tick, INSTALL_POLL_MS);
    };
    timer = window.setTimeout(tick, INSTALL_POLL_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [watching, refresh, mode]);

  const prepare = useCallback(async () => {
    setLocal({ ...IDLE, busy: true });
    const { app: current } = latest.current;
    try {
      const build = await api.prepare(current.source_id, current.bundle_id, prepareBody);
      if (isActiveBuild(build)) {
        setLocal({ ...IDLE, build });
        // Other buttons for this app (Browse's Fresh card and its row) adopt the build too.
        if (mode === "app") latest.current.onState({ ...latest.current.app.state, action: "preparing", build });
      } else await settle(build);
      reloadMe();
    } catch (error) {
      setLocal({ ...IDLE, error: errorMessage(error) });
    }
  }, [mode, prepareBody, settle, reloadMe]);

  /** Called from the install link's click handler; the link itself performs the navigation. */
  const installTapped = useCallback((href: string) => {
    setLocal((l) => ({ ...l, watching: true }));
    if (import.meta.env.VITE_MOCK === "1") simulateManifestFetch(href);
  }, []);

  return { local, prepare, installTapped };
}

/** Mock mode only: desktop browsers ignore itms-services links, so fetch the manifest like iOS would. */
function simulateManifestFetch(href: string) {
  try {
    const manifest = new URL(href).searchParams.get("url");
    if (manifest) void fetch(manifest).catch(() => {});
  } catch {
    // Not a URL; nothing to simulate.
  }
}
