import type { AppState, AppSummary, Build, Installed, Release } from "../api/types";
import { formatVersion } from "./format";

/** Client-side progress of one action button, layered over the server's AppState. */
export interface LocalFlow {
  /** Build this button is preparing (polled until ready or failed). */
  build: Build | null;
  /** Error from the prepare request itself. */
  error: string | null;
  /** Prepare request in flight. */
  busy: boolean;
  /** The user tapped an install link; app state is being polled. */
  watching: boolean;
}

export const IDLE: LocalFlow = { build: null, error: null, busy: false, watching: false };

export type ButtonView =
  | { kind: "prepare"; label: "Get" | "Update" | "Retry" | "Install"; caption: string | null; error: boolean }
  | { kind: "progress"; progress: number | null; caption: string | null }
  | { kind: "link"; label: "Install" | "Update" | "Reinstall"; href: string; caption: string | null }
  | { kind: "installing"; href: string | null }
  | { kind: "open"; href: string }
  | { kind: "disabled"; label: string; caption: string | null };

export function isActiveBuild(build: Build | null | undefined): boolean {
  return !!build && build.status !== "ready" && build.status !== "failed";
}

const STATUS_LABELS: Record<Build["status"], string> = {
  queued: "Waiting",
  downloading: "Downloading",
  signing: "Signing",
  packaging: "Packaging",
  ready: "Ready",
  failed: "Failed",
};

export function buildStepLabel(build: Build): string {
  return build.step || STATUS_LABELS[build.status];
}

function progressView(build: Build | null): ButtonView {
  const progress = build?.progress;
  return {
    kind: "progress",
    progress: typeof progress === "number" && Number.isFinite(progress) ? Math.min(Math.max(progress, 0), 1) : null,
    caption: build ? buildStepLabel(build) : "Preparing",
  };
}

function failedView(error: string | null | undefined): ButtonView {
  return { kind: "prepare", label: "Retry", caption: error || "Preparing failed", error: true };
}

/** Actions whose app state still points at an install link (or the install in progress). */
export function isInstallPending(state: AppState): boolean {
  return state.action === "install" || state.action === "update" || state.action === "installing";
}

/** What the main action button of an app shows. */
export function deriveAppButton(app: Pick<AppSummary, "unavailable" | "state">, local: LocalFlow): ButtonView {
  const { state } = app;

  if (local.busy) return progressView(null);
  if (local.error) return failedView(local.error);

  if (local.build && isActiveBuild(local.build)) return progressView(local.build);

  // A build this button prepared finished before the app state caught up.
  const stale = state.action === "get" || state.action === "retry" || state.action === "preparing" ||
    (state.action === "update" && !state.install_url);
  if (local.build && stale) {
    if (local.build.status === "failed") return failedView(local.build.error);
    if (local.build.install_url) {
      if (local.watching) return { kind: "installing", href: local.build.install_url };
      return { kind: "link", label: state.installed ? "Update" : "Install", href: local.build.install_url, caption: null };
    }
    return progressView(local.build);
  }

  if (local.watching && isInstallPending(state)) return { kind: "installing", href: state.install_url };

  switch (state.action) {
    case "get":
      return { kind: "prepare", label: "Get", caption: null, error: false };
    case "retry":
      return failedView(state.build?.error);
    case "update": {
      const caption = state.update_reason === "resign" ? "Re-sign" : null;
      if (state.install_url) return { kind: "link", label: "Update", href: state.install_url, caption };
      return { kind: "prepare", label: "Update", caption, error: false };
    }
    case "install":
      if (state.install_url) return { kind: "link", label: "Install", href: state.install_url, caption: null };
      return { kind: "prepare", label: "Get", caption: null, error: false };
    case "preparing":
      return progressView(state.build);
    case "installing":
      return { kind: "installing", href: state.install_url };
    case "open":
      if (state.open_url) return { kind: "open", href: state.open_url };
      return { kind: "disabled", label: "Installed", caption: null };
    case "unavailable":
      // Greyed out like the App Store; the reason is shown next to it.
      return {
        kind: "disabled",
        label: "Get",
        caption: app.unavailable ?? state.target?.incompatibility ?? null,
      };
  }
}

export function isInstalledRelease(release: Pick<Release, "version" | "build" | "revision">, installed: Installed | null): boolean {
  if (!installed || installed.version !== release.version) return false;
  if (release.revision !== installed.revision) return false;
  return !release.build || !installed.build || release.build === installed.build;
}

/** The "Install this version" button of a version history row. */
export function deriveVersionButton(release: Release, installed: Installed | null, local: LocalFlow): ButtonView {
  if (isInstalledRelease(release, installed)) return { kind: "disabled", label: "Installed", caption: null };
  if (!release.compatible) return { kind: "disabled", label: "Incompatible", caption: release.incompatibility };
  if (local.busy) return progressView(null);
  if (local.error) return failedView(local.error);
  const { build } = local;
  if (build) {
    if (isActiveBuild(build)) return progressView(build);
    if (build.status === "failed") return failedView(build.error);
    if (build.install_url) {
      if (local.watching) return { kind: "installing", href: build.install_url };
      return { kind: "link", label: "Install", href: build.install_url, caption: null };
    }
  }
  return { kind: "prepare", label: "Install", caption: null, error: false };
}

/**
 * Kiwi can't see an app deleted on the iPhone (without devicectl), so it still offers Open.
 * Reinstall offers it again: an app that is installed and up to date, with a release to install.
 */
export function canReinstall(app: Pick<AppSummary, "unavailable" | "state">): boolean {
  return app.state.action === "open" && !!app.state.installed && !!app.state.target && !app.unavailable;
}

/** The app page's button while reinstalling: it prepares the target release right away. */
export function deriveReinstallButton(local: LocalFlow): ButtonView {
  if (local.error) return failedView(local.error);
  const { build } = local;
  if (!build || isActiveBuild(build)) return progressView(build);
  // Failed, or ready but expired since: Retry prepares a new build.
  if (!build.install_url) return failedView(build.error);
  if (local.watching) return { kind: "installing", href: build.install_url };
  return { kind: "link", label: "Reinstall", href: build.install_url, caption: null };
}

/**
 * The installed version and the version Update installs ("1.0" → "2.0"), while an update to a
 * different release is offered, preparing, ready, installing or failed. Re-signs of the same
 * release have none. Builds are shown when the versions alone are equal.
 */
export function versionChange(
  state: Pick<AppState, "action" | "installed" | "target">,
): { from: string; to: string } | null {
  const { action, installed, target } = state;
  if (!installed || !target || action === "open" || action === "unavailable" || action === "get") return null;
  if (isInstalledRelease(target, installed)) return null;
  if (installed.version !== target.version) return { from: installed.version, to: target.version };
  return { from: formatVersion(installed), to: formatVersion(target) };
}

/** Updates that still need preparing (as opposed to ready, preparing or installing). */
export function needsPrepare(state: Pick<AppState, "action" | "install_url">): boolean {
  return (state.action === "update" && !state.install_url) || state.action === "retry";
}

/** Replaces the state of one app in a list (apps are keyed by source and bundle id). */
export function withAppState<T extends AppSummary>(list: T[], app: AppSummary, state: AppState): T[] {
  return list.map((item) =>
    item.source_id === app.source_id && item.bundle_id === app.bundle_id ? { ...item, state } : item,
  );
}

export function appKey(app: Pick<AppSummary, "source_id" | "bundle_id">): string {
  return `${app.source_id}/${app.bundle_id}`;
}

/**
 * Library rows are installs: one per signed bundle id. A devicectl sync can list two installs
 * of the same source app (e.g. Kiwi's build and the App Store build), so the app key is not unique.
 */
export function libraryKey(app: Pick<AppSummary, "source_id" | "bundle_id" | "state">): string {
  return app.state.installed?.signed_bundle_id ?? appKey(app);
}

/** Replaces the state of one library row (rows are keyed by signed bundle id). */
export function withLibraryState<T extends AppSummary>(list: T[], app: AppSummary, state: AppState): T[] {
  const key = libraryKey(app);
  return list.map((item) => (libraryKey(item) === key ? { ...item, state } : item));
}

/** "Replaces YouTubePlus (YTLite Repository)" when installing would replace another source's app. */
export function replacesLabel(state: Pick<AppState, "replaces">): string | null {
  const r = state.replaces;
  return r ? `Replaces ${r.name} (${r.source_name})` : null;
}
