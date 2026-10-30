import type { Me, PersonApp, Registration } from "../api/types";

/** localStorage flags: the Developer Mode banner was dismissed, the guide was shown once. */
export const DEV_MODE_BANNER = "kiwi.devModeBannerDismissed";
export const DEV_MODE_SHOWN = "kiwi.devModeGuideShown";

/** Marketing names of recent iPhones by machine identifier. */
const MODELS: Record<string, string> = {
  "iPhone13,1": "iPhone 12 mini",
  "iPhone13,2": "iPhone 12",
  "iPhone13,3": "iPhone 12 Pro",
  "iPhone13,4": "iPhone 12 Pro Max",
  "iPhone14,4": "iPhone 13 mini",
  "iPhone14,5": "iPhone 13",
  "iPhone14,2": "iPhone 13 Pro",
  "iPhone14,3": "iPhone 13 Pro Max",
  "iPhone14,6": "iPhone SE",
  "iPhone14,7": "iPhone 14",
  "iPhone14,8": "iPhone 14 Plus",
  "iPhone15,2": "iPhone 14 Pro",
  "iPhone15,3": "iPhone 14 Pro Max",
  "iPhone15,4": "iPhone 15",
  "iPhone15,5": "iPhone 15 Plus",
  "iPhone16,1": "iPhone 15 Pro",
  "iPhone16,2": "iPhone 15 Pro Max",
  "iPhone17,1": "iPhone 16 Pro",
  "iPhone17,2": "iPhone 16 Pro Max",
  "iPhone17,3": "iPhone 16",
  "iPhone17,4": "iPhone 16 Plus",
  "iPhone17,5": "iPhone 16e",
  "iPhone18,1": "iPhone 17 Pro",
  "iPhone18,2": "iPhone 17 Pro Max",
  "iPhone18,3": "iPhone 17",
  "iPhone18,4": "iPhone Air",
};

/** "iPhone", "iPad" or "iPod touch" for a machine identifier. */
export function deviceKind(product: string | null | undefined): string {
  if (product?.startsWith("iPad")) return "iPad";
  if (product?.startsWith("iPod")) return "iPod touch";
  return "iPhone";
}

/** "iPhone 15" for "iPhone15,4"; the kind of device for identifiers this list doesn't know. */
export function modelName(product: string | null | undefined): string {
  return (product && MODELS[product]) ?? deviceKind(product);
}

export function firstName(name: string): string {
  return name.trim().split(/\s+/)[0] || name;
}

/** "Noah's" (no extra s after a final s: "James'"). */
export function possessive(name: string): string {
  const first = firstName(name);
  return first.endsWith("s") ? `${first}'` : `${first}'s`;
}

export function isMember(me: Me | null | undefined): boolean {
  return me?.access.role === "member";
}

/** Where a member's device stands before it may install apps. */
export type GateStep = "welcome" | "declined" | "waiting" | "ready";

export function gateStep(registration: Registration, enrolled: boolean): GateStep {
  if (registration.ready) return "ready";
  switch (registration.status) {
    case "pending":
    case "approved":
      return "waiting";
    case "declined":
      return "declined";
    default:
      // Back from Settings: iOS sent the ID; the server may answer a moment later.
      return enrolled ? "waiting" : "welcome";
  }
}

/**
 * The order apps are listed in on a person's page: the ones they may use or have installed,
 * then (when shown) the rest, each by name. Computed once so rows don't jump when toggled.
 */
export function featuredKeys(apps: PersonApp[]): Set<string> {
  return new Set(apps.filter((a) => a.allowed || a.installed_version).map(personAppKey));
}

export function personAppKey(app: Pick<PersonApp, "source_id" | "bundle_id">): string {
  return `${app.source_id}/${app.bundle_id}`;
}
