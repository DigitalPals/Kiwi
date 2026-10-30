export function isIOS(): boolean {
  const ua = navigator.userAgent;
  // iPadOS reports a Mac user agent but has touch.
  return /iPhone|iPad|iPod/.test(ua) || (/Macintosh/.test(ua) && navigator.maxTouchPoints > 1);
}

/** True when running from the Home Screen. */
export function isStandalone(): boolean {
  return navigator.standalone === true || window.matchMedia("(display-mode: standalone)").matches;
}

/** Only iOS Safari defines `navigator.standalone`; it is false in a browser tab. */
export function shouldSuggestHomeScreen(): boolean {
  return isIOS() && navigator.standalone === false;
}

export async function syncBadge(count: number): Promise<void> {
  if (!("setAppBadge" in navigator)) return;
  try {
    if (count > 0) await navigator.setAppBadge(count);
    else await navigator.clearAppBadge();
  } catch {
    // Not permitted (e.g. Safari tab without notification permission).
  }
}

export function registerServiceWorker() {
  if (!import.meta.env.PROD || !("serviceWorker" in navigator)) return;
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("/sw.js", { scope: "/" }).catch((error: unknown) => {
      console.warn("Service worker registration failed", error);
    });
  });
}
