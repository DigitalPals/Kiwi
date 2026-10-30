import type { AppSummary } from "../api/types";
import { versionChange } from "./action";
import { parseDate } from "./format";

const WEEK = 7 * 86_400_000;
const MAX = 10;

/** A newer version of an installed app, also while it is preparing or installing. */
function isUpdate(app: AppSummary): boolean {
  return versionChange(app.state) !== null;
}

/**
 * Browse's "Fresh This Week" shelf: apps whose release came out in the last week and that
 * still need something (get, update, install), updates first, then the newest.
 */
export function freshApps(apps: readonly AppSummary[], now: Date = new Date()): AppSummary[] {
  const released = (app: AppSummary) => parseDate(app.state.target?.date)?.getTime() ?? null;
  return apps
    .filter((app) => {
      const { action, update_reason } = app.state;
      if (app.hidden || action === "open" || action === "unavailable") return false;
      if (action === "update" && update_reason === "resign") return false;
      const date = released(app);
      return date !== null && now.getTime() - date <= WEEK;
    })
    .sort((a, b) => Number(isUpdate(b)) - Number(isUpdate(a)) || (released(b) ?? 0) - (released(a) ?? 0))
    .slice(0, MAX);
}

/** The small label on a Fresh card. */
export function freshLabel(app: AppSummary): string {
  const { action, install_url } = app.state;
  if (isUpdate(app)) return install_url && action === "update" ? "Update Ready" : "New Version";
  if (action === "install" && install_url) return "Ready to Install";
  return "New Release";
}
