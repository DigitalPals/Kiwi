export type Tab = "browse" | "apps" | "sources" | "settings";

export type Route =
  | { name: "browse" }
  | { name: "apps" }
  | { name: "sources" }
  | { name: "settings" }
  | { name: "signing" }
  | { name: "device" }
  | { name: "notifications" }
  | { name: "builds" }
  | { name: "build"; app: string }
  | { name: "app"; sourceId: string; bundleId: string }
  | { name: "source"; sourceId: string }
  | { name: "family" }
  | { name: "person"; personId: string }
  | { name: "family-device"; deviceId: string }
  | { name: "apple" }
  | { name: "not-found"; path: string };

export const TABS: readonly { tab: Tab; path: string; label: string }[] = [
  { tab: "browse", path: "/", label: "Browse" },
  { tab: "apps", path: "/apps", label: "My Apps" },
  { tab: "sources", path: "/sources", label: "Sources" },
  { tab: "settings", path: "/settings", label: "Settings" },
];

/** Members have no Sources tab. */
export function tabsFor(member: boolean): readonly { tab: Tab; path: string; label: string }[] {
  return member ? TABS.filter((t) => t.tab !== "sources") : TABS;
}

export const FAMILY_PATH = "/settings/family";
export const APPLE_PATH = "/settings/family/apple";
export const SIGNING_PATH = "/settings/signing";
export const DEVICE_PATH = "/settings/device";
export const NOTIFICATIONS_PATH = "/settings/notifications";
/** Kiwi Builds, under Sources. */
export const BUILDS_PATH = "/builds";

export const buildPath = (app: string) => `${BUILDS_PATH}/${encodeURIComponent(app)}`;

/** Routes only the owner can use. */
export function ownerOnly(route: Route): boolean {
  return ["sources", "source", "builds", "build", "family", "person", "family-device", "apple", "signing"].includes(
    route.name,
  );
}

function decode(segment: string): string | null {
  try {
    return decodeURIComponent(segment);
  } catch {
    return null;
  }
}

export function matchRoute(pathname: string): Route {
  const trimmed = pathname.length > 1 ? pathname.replace(/\/+$/, "") : pathname;
  switch (trimmed) {
    case "":
    case "/":
      return { name: "browse" };
    case "/apps":
    // Older links and notifications: My Apps replaced the Updates and Library tabs.
    case "/updates":
    case "/library":
      return { name: "apps" };
    case "/sources":
      return { name: "sources" };
    case "/settings":
      return { name: "settings" };
    case SIGNING_PATH:
      return { name: "signing" };
    case DEVICE_PATH:
      return { name: "device" };
    case NOTIFICATIONS_PATH:
      return { name: "notifications" };
    case BUILDS_PATH:
    // Earlier links: the YouTube-only build settings, then Kiwi Builds under Settings.
    case "/automation":
    case "/settings/builds":
      return { name: "builds" };
    case FAMILY_PATH:
      return { name: "family" };
    case APPLE_PATH:
      return { name: "apple" };
  }
  const parts = trimmed.split("/");
  // ["", "builds", app], or ["", "settings", "builds", app] from earlier links
  const buildSegment =
    parts.length === 3 && parts[1] === "builds"
      ? parts[2]
      : parts.length === 4 && parts[1] === "settings" && parts[2] === "builds"
        ? parts[3]
        : null;
  if (buildSegment) {
    const app = decode(buildSegment);
    if (app) return { name: "build", app };
  }
  // ["", "settings", "family", "devices", deviceId] (push notifications link here)
  if (parts.length === 5 && parts[1] === "settings" && parts[2] === "family" && parts[3] === "devices" && parts[4]) {
    const deviceId = decode(parts[4]);
    if (deviceId) return { name: "family-device", deviceId };
  }
  // ["", "settings", "family", personId]
  if (parts.length === 4 && parts[1] === "settings" && parts[2] === "family" && parts[3] && parts[3] !== "devices") {
    const personId = decode(parts[3]);
    if (personId) return { name: "person", personId };
  }
  // ["", "sources", sourceId]
  if (parts.length === 3 && parts[1] === "sources" && parts[2]) {
    const sourceId = decode(parts[2]);
    if (sourceId) return { name: "source", sourceId };
  }
  // ["", "app", sourceId, bundleId]
  if (parts.length === 4 && parts[1] === "app" && parts[2] && parts[3]) {
    const sourceId = decode(parts[2]);
    const bundleId = decode(parts[3]);
    if (sourceId && bundleId) return { name: "app", sourceId, bundleId };
  }
  return { name: "not-found", path: pathname };
}

export function appPath(sourceId: string, bundleId: string): string {
  return `/app/${encodeURIComponent(sourceId)}/${encodeURIComponent(bundleId)}`;
}

export function sourcePath(sourceId: string): string {
  return `/sources/${encodeURIComponent(sourceId)}`;
}

export function personPath(personId: string): string {
  return `${FAMILY_PATH}/${encodeURIComponent(personId)}`;
}

export function familyDevicePath(deviceId: string): string {
  return `${FAMILY_PATH}/devices/${encodeURIComponent(deviceId)}`;
}

export function routeTab(route: Route): Tab | null {
  switch (route.name) {
    case "browse":
    case "apps":
    case "sources":
    case "settings":
      return route.name;
    case "source":
    case "builds":
    case "build":
      return "sources";
    case "signing":
    case "device":
    case "notifications":
    case "family":
    case "person":
    case "family-device":
    case "apple":
      return "settings";
    default:
      return null;
  }
}

export interface ClickLike {
  button: number;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  defaultPrevented: boolean;
}

/**
 * Returns the in-app path for a link click that the router should handle, or null when the
 * browser should follow the link itself (other origin, other scheme such as itms-services,
 * modifier keys, explicit target or download).
 */
export function interceptedPath(
  event: ClickLike,
  link: { href: string; target: string; hasDownload: boolean },
  origin: string,
): string | null {
  if (event.defaultPrevented || event.button !== 0) return null;
  if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return null;
  if ((link.target && link.target !== "_self") || link.hasDownload) return null;
  let url: URL;
  try {
    url = new URL(link.href, origin);
  } catch {
    return null;
  }
  if (url.origin !== origin) return null;
  // The server answers these itself (OTA installs, enrollment profiles).
  if (url.pathname === "/api" || ["/api/", "/ota/", "/enroll/"].some((p) => url.pathname.startsWith(p))) return null;
  return url.pathname + url.search + url.hash;
}
