import type { Device, Me, Platform, Signing, Warning } from "../api/types";
import { expiryInfo, formatDate } from "./format";

/** A browser that doesn't tell its platform, such as a desktop browser. */
export const isUndetected = (device: Pick<Device, "detected_platform">): boolean => device.detected_platform === null;

/**
 * The platform the first question starts with: an earlier choice (Show Setup Again), else
 * what Kiwi detected. Nothing on a desktop browser, so Continue waits for a choice.
 */
export function initialChoice(device: Pick<Device, "platform" | "platform_source" | "detected_platform">): Platform | null {
  return device.platform_source === "chosen" ? device.platform : device.detected_platform;
}

/** The second page of the setup, after the platform is chosen. */
export type SetupPage = "iphone" | "android";

/**
 * The page that sets up a platform, or null when there is nothing to set up and the setup
 * ends with the choice: a desktop browser can't become a Home Screen app or install the
 * Kiwi app, and Android has nothing to set up while Kiwi serves no Android app. Like the
 * server's `android_app_offer`, the Kiwi app is offered only on a device detected as
 * Android: no other device can install it.
 */
export function setupPage(platform: Platform, me: Pick<Me, "device" | "android_app">): SetupPage | null {
  if (isUndetected(me.device)) return null;
  if (platform === "ios") return "iphone";
  return me.android_app && me.device.detected_platform === "android" ? "android" : null;
}

/** Whether choosing Android in the setup offers this device the Kiwi app (see `setupPage`). */
export const offersKiwiApp = (me: Pick<Me, "device" | "android_app">): boolean => setupPage("android", me) !== null;

export interface SigningRow {
  tone: "ok" | "warn" | "off";
  title: string;
  sub: string | null;
}

/**
 * The iPhone setup's signing row: whether this Kiwi can sign apps for the iPhone, from the
 * warnings Settings shows (no identity or profile) and, for the date, `active` from
 * `/api/signing` (undefined while it loads).
 */
export function signingRow(warnings: readonly Warning[], active: Signing["active"] | undefined, now = new Date()): SigningRow {
  const blocked = warnings.some((w) => w.level === "error" && w.link === "/settings");
  if (blocked || active === null) {
    return { tone: "warn", title: "Signing isn't set up on this Kiwi yet", sub: "Set it up in Settings › Signing to install apps." };
  }
  if (active === undefined) return { tone: "off", title: "Checking signing…", sub: null };
  const expiry = expiryInfo(active.expires_at, now);
  const until = `This Kiwi signs apps for this iPhone until ${formatDate(active.expires_at)}`;
  if (expiry && expiry.tone !== "ok") return { tone: "warn", title: `Signing ${expiry.label.toLowerCase()}`, sub: until };
  return { tone: "ok", title: "Signing ready", sub: until };
}
