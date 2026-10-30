import type { Me, Warning } from "../api/types";
import type { TabDots } from "../components/TabBar";

/**
 * Warnings show where they are fixed instead of in banners on every screen: failed sources
 * in Sources, signing and the iOS version in Settings, expiring apps in My Apps (whose badge
 * already counts their re-sign updates). Tabs with something to fix get a dot.
 */
export function tabDots(me: Me | null): TabDots {
  if (!me) return {};
  const dots: TabDots = {};
  if (me.counts.source_errors > 0) dots.sources = "warn";
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
