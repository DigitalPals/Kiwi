import type { Me, Warning } from "../api/types";
import type { TabDots } from "../components/TabBar";

/**
 * Warnings show where they are fixed instead of in banners on every screen: failed sources
 * in Sources, signing and the iOS version in Settings (and anything else Settings links to,
 * such as Kiwi's Android app on Android), expiring apps in My Apps (whose badge already
 * counts their re-sign updates). Tabs with something to fix get a dot, and the tab shows why.
 * Android has no Sources tab: its sources list iPhone apps.
 */
export function tabDots(me: Me | null): TabDots {
  if (!me) return {};
  const dots: TabDots = {};
  if (me.counts.source_errors > 0 && !me.features.includes("android_apps")) dots.sources = "warn";
  const settings = settingsWarnings(me);
  if (settings.some((w) => w.level === "error")) dots.settings = "error";
  else if (settings.some((w) => w.level === "warning")) dots.settings = "warn";
  return dots;
}

/** Signing problems and the unknown iOS version, which Settings shows. */
export function settingsWarnings(me: Me): Warning[] {
  return me.warnings.filter((w) => w.link === "/settings");
}

/** Problems that stop every install (no identity or profile): Browse shows these too. */
export function blockingWarnings(me: Me): Warning[] {
  return me.warnings.filter((w) => w.level === "error");
}

/**
 * The Settings warnings an iPhone's Settings shows in rows of its own: signing problems in
 * Kiwi's card and the Signing row, the unknown iOS version in This iPhone ("iOS unknown").
 */
const IPHONE_SETTINGS_ROWS = new Set(["no_identity", "no_profile", "expiring", "unknown_os"]);

/**
 * Settings warnings without a row of their own, which Settings lists with their message so
 * that every dot on its tab has a reason there: on Android, Kiwi's Android app that couldn't
 * be prepared (`android_app`).
 */
export function listedSettingsWarnings(me: Me): Warning[] {
  const ownRows = me.features.includes("ios_apps") ? IPHONE_SETTINGS_ROWS : new Set<string>();
  return settingsWarnings(me).filter((w) => !ownRows.has(w.code));
}
