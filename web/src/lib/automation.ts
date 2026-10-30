import type { AutomationJob, AutomationOverview, AutomationPreferences, AutomationState } from "../api/types";
import type { IconName } from "../components/Icon";
import { formatVersion, plural } from "./format";
import { appPath } from "./routes";

/** The built-in source; its id predates the apps other than YouTube. */
export const BUILD_SOURCE_ID = "kiwi-youtube";

export const builtAppPath = (state: Pick<AutomationState, "bundle_id">) => appPath(BUILD_SOURCE_ID, state.bundle_id);

export const JOB_LABELS = {
  queued: "Queued",
  running: "Building",
  ready: "Ready",
  unchanged: "Up to date",
  failed: "Failed",
  blocked: "Needs attention",
  cancelled: "Cancelled",
} as const;

/** The icon of a known limitation; unknown names get "info". */
export function limitationIcon(icon: string): IconName {
  const known: IconName[] = ["key", "bell", "clock", "refresh", "phone", "share", "info", "warning"];
  return known.includes(icon as IconName) ? (icon as IconName) : "info";
}

export function samePreferences(a: AutomationPreferences, b: AutomationPreferences): boolean {
  return a.enabled === b.enabled && a.interval_minutes === b.interval_minutes && (a.name ?? null) === (b.name ?? null)
    && [...a.tweaks].sort().join(",") === [...b.tweaks].sort().join(",");
}

export function intervalLabel(minutes: number): string {
  if (minutes === 1440) return "Every day";
  if (minutes === 60) return "Every hour";
  return minutes % 60 === 0 ? `Every ${minutes / 60} hours` : `Every ${minutes} minutes`;
}

export function activeJob(state: Pick<AutomationState, "jobs">): AutomationJob | undefined {
  return state.jobs.find((j) => !j.finished_at);
}

const needsAttention = (state: Pick<AutomationState, "jobs">) =>
  state.jobs[0]?.status === "failed" || state.jobs[0]?.status === "blocked";

/** A paused schedule can still have a manual build running or a working release. */
export function automationSummary(state: AutomationState): string {
  const active = activeJob(state);
  if (active) return active.stage;
  if (needsAttention(state)) return "Needs attention";
  if (state.configuration_changed) return "Saved changes ready to build";
  if (!state.preferences.enabled) return "Automatic builds off";
  if (!state.published) return "Ready for your first build";
  return intervalLabel(state.preferences.interval_minutes);
}

/** One line for an added app in Kiwi Builds: the available build and how it's doing. */
export function statusLine(state: AutomationState): string {
  const active = activeJob(state);
  if (active) return active.stage;
  if (!state.published) return automationSummary(state);
  const version = formatVersion({ ...state.published, revision: state.published.revision ?? undefined });
  if (needsAttention(state)) return `${version} · needs attention`;
  if (state.configuration_changed) return `${version} · changes to build`;
  return `${version} · ${state.preferences.enabled ? "up to date" : "automatic builds off"}`;
}

/** The Settings row's summary of all builds. */
export function overviewSummary(overview: AutomationOverview): string {
  const added = overview.apps.filter((a) => a.added);
  const running = added.find((a) => activeJob(a)?.status === "running");
  if (running) return `${running.name}: ${activeJob(running)!.stage}`;
  const attention = added.find(needsAttention);
  if (attention) return `${attention.name} needs attention`;
  if (added.length === 0) return "Patch and sign App Store apps on your Mac";
  return added.length <= 2 ? added.map((a) => a.name).join(" and ") : plural(added.length, "app");
}

/** The app's tweak and the version it is built on, e.g. "NeoFreeBird 7.3.0 · X 12.38.1". */
export function heroLine(state: AutomationState): string {
  const version = state.app_version ?? state.published?.version;
  return version ? `${state.tweak} · ${state.name} ${version}` : state.tweak;
}

export function selectedFeatures(state: AutomationState, tweaks: string[], name?: string | null): string {
  return [
    ...state.included.map((f) => f.name),
    ...state.available_tweaks.filter((t) => tweaks.includes(t.id)).map((t) => t.name),
    ...(name && state.names.length > 0 ? [`Named ${name}`] : []),
  ].join(" · ");
}
