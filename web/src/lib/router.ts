import { useEffect, useState } from "preact/hooks";
import { interceptedPath, matchRoute, routeTab, type Route, type Tab } from "./routes";

interface HistoryEntry {
  idx: number;
}

const listeners = new Set<() => void>();
const scrollPositions = new Map<number, number>();
let pendingScroll: number | null = null;
let lastTab: Tab = "browse";

function currentPath(): string {
  return location.pathname + location.search;
}

function currentIdx(): number {
  const state = history.state as HistoryEntry | null;
  return typeof state?.idx === "number" ? state.idx : 0;
}

function notify() {
  for (const listener of listeners) listener();
}

export function navigate(path: string, options: { replace?: boolean } = {}) {
  if (path === currentPath() && !options.replace) {
    window.scrollTo({ top: 0, behavior: "smooth" });
    return;
  }
  scrollPositions.set(currentIdx(), window.scrollY);
  if (options.replace) {
    history.replaceState({ idx: currentIdx() } satisfies HistoryEntry, "", path);
  } else {
    history.pushState({ idx: currentIdx() + 1 } satisfies HistoryEntry, "", path);
  }
  pendingScroll = 0;
  notify();
}

/** Goes back within the app, or to `fallback` when the page was opened directly. */
export function goBack(fallback = "/") {
  if (currentIdx() > 0) history.back();
  else navigate(fallback, { replace: true });
}

export function previousTab(): Tab {
  return lastTab;
}

/** Restores the scroll position saved for the current entry once content is tall enough. */
export function applyPendingScroll() {
  if (pendingScroll === null) return;
  const target = pendingScroll;
  pendingScroll = null;
  let attempts = 0;
  const tryScroll = () => {
    const max = document.documentElement.scrollHeight - window.innerHeight;
    window.scrollTo(0, Math.min(target, Math.max(max, 0)));
    if (max < target && attempts++ < 20) window.setTimeout(tryScroll, 50);
  };
  tryScroll();
}

export function startRouter() {
  history.scrollRestoration = "manual";
  if (typeof (history.state as HistoryEntry | null)?.idx !== "number") {
    history.replaceState({ idx: 0 } satisfies HistoryEntry, "");
  }

  window.addEventListener("popstate", () => {
    pendingScroll = scrollPositions.get(currentIdx()) ?? 0;
    notify();
  });

  window.addEventListener("scroll", () => scrollPositions.set(currentIdx(), window.scrollY), { passive: true });

  document.addEventListener("click", (event) => {
    const anchor = (event.target as Element | null)?.closest?.("a");
    if (!anchor || !anchor.getAttribute("href")) return;
    const path = interceptedPath(
      event,
      { href: anchor.href, target: anchor.target, hasDownload: anchor.hasAttribute("download") },
      location.origin,
    );
    if (path === null) return;
    event.preventDefault();
    navigate(path);
  });

  // The service worker asks an open window to show a notification's URL.
  if ("serviceWorker" in navigator) {
    navigator.serviceWorker.addEventListener("message", (event: MessageEvent<unknown>) => {
      const data = event.data as { type?: unknown; url?: unknown } | null;
      if (data?.type === "navigate" && typeof data.url === "string") {
        const url = new URL(data.url, location.origin);
        if (url.origin === location.origin) navigate(url.pathname + url.search);
      }
    });
    navigator.serviceWorker.startMessages();
  }
}

export function useRoute(): Route {
  const [path, setPath] = useState(currentPath);
  useEffect(() => {
    const listener = () => setPath(currentPath());
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }, []);
  const route = matchRoute(path.split("?")[0] ?? "/");
  const tab = routeTab(route);
  if (tab) lastTab = tab;
  return route;
}
