import type {
  AutomationJob, AutomationOverview, AutomationPreferences, AutomationState, BuilderInfo, Host, TweakPins, TweakSource,
} from "../api/types";
import type { IconName } from "../components/Icon";
import { formatDate, formatVersion, plural } from "./format";
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

/**
 * Whether this Kiwi builds iOS apps: on a Mac, or elsewhere with a paired Mac builder
 * (without one the server answers `409 needs_mac`).
 */
export const buildsIos = (host: Pick<Host, "os">, info?: Pick<BuilderInfo, "builder"> | null): boolean =>
  host.os === "macos" || !!info?.builder;

const GOOGLE_PLAY =
  "Kiwi will download apps from Google Play, patch them with open-source patches and sign them with this Kiwi's own key.";

/**
 * What turning Kiwi Builds on means, for its confirmation: on an iPhone the App Store apps it
 * downloads with the owner's Apple account, on Android the Google Play apps. A Kiwi that
 * doesn't run on macOS builds iOS apps on its paired Mac builder; without one it builds
 * Android apps only, which an iPhone is told plainly.
 */
export function buildsNotice(android: boolean, host: Pick<Host, "os">, info?: Pick<BuilderInfo, "builder"> | null): string {
  let what = GOOGLE_PLAY;
  let setup = "";
  if (!android && buildsIos(host, info)) {
    const mac = host.os === "macos" ? "this Mac" : info!.builder!.name;
    what = `Kiwi will download App Store apps with your Apple account, decrypt them on ${mac} and patch them with open-source tweaks.`;
    setup = "\n\nThe Mac needs a one-time setup first (see Kiwi's documentation).";
  } else if (!android) {
    setup = "\n\niOS apps need a Mac builder. Until you pair a Mac in Settings › Mac Builder, this Kiwi builds Android apps only.";
  }
  return (
    `Turn on Kiwi Builds?\n\n${what} Patched apps may break the app's terms of service. ` +
    `Use it only for apps you may use, on your own devices, at your own risk.${setup}`
  );
}

/** Kiwi Builds' apps for this device: Android recipes on Android, the iPhone's elsewhere. */
export function appsForDevice(apps: readonly AutomationState[], android: boolean): AutomationState[] {
  return apps.filter((app) => (app.platform === "android") === android);
}

/** A commit as people read it. */
export const shortSha = (sha: string): string => sha.slice(0, 7);

/**
 * Where a followed package's source comes from, e.g. "Release v2026.45.0-1.3.8 · 10 Nov" or
 * "main · 217c79f · 10 Nov"; sources this Kiwi version pins say so.
 */
export function sourceLine(source: TweakSource): string {
  if (!source.updated) {
    const follows = source.follows === "release" ? "follows releases" : `follows ${source.branch ?? "its branch"}`;
    return `Kiwi's pinned commit ${shortSha(source.sha)} · ${follows}`;
  }
  const date = source.date ? ` · ${formatDate(source.date)}` : "";
  return source.follows === "release" && source.tag
    ? `Release ${source.tag}${date}`
    : `${source.branch ?? "Branch"} · ${shortSha(source.sha)}${date}`;
}

/** The packages newer sources change, by name: "RedditFilter 1.4.0", "YouMod a1b2c3d". */
export function pinsLabel(state: AutomationState, pins: TweakPins): string {
  const names = (state.tweak_sources ?? []).flatMap((s) => {
    const pin = pins[s.id];
    return pin && pin.sha !== s.sha ? [`${s.name} ${pin.version ?? shortSha(pin.sha)}`] : [];
  });
  return names.length ? names.join(", ") : "Newer sources";
}
