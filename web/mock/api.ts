// In-memory imitation of kiwi-server for `VITE_MOCK=1 npm run dev`. Follows docs/api.md and the
// action rules in docs/architecture.md closely enough to exercise every UI state.
import { createHash, generateKeyPairSync, randomBytes } from "node:crypto";
import type {
  AndroidApp,
  AppDetail,
  AppState,
  AppSummary,
  AutomationJob,
  AutomationPreferences,
  AutomationState,
  Build,
  Client,
  Device,
  DevicesResponse,
  Feature,
  Host,
  Installed,
  Me,
  Original,
  OriginalMatch,
  OtherVersion,
  Platform,
  Release,
  Settings,
  SetupCheck,
  Signing,
  SigningBackend,
  Source,
  StoreApp,
  Warning,
} from "../src/api/types.ts";
import {
  CERT_EXPIRES,
  DAY,
  PROFILE_EXPIRES,
  TEAM_ID,
  createDb,
  type MockApp,
  type MockAttempt,
  type MockBuild,
  type MockDb,
  type MockRelease,
  type MockSource,
  type MockStoreApp,
} from "./fixtures.ts";
import { buildStages, catalogOf, createBuilds, iconBundleId } from "./builds.ts";

export interface MockRequest {
  method: string;
  path: string;
  query: URLSearchParams;
  headers: Record<string, string | undefined>;
  body: string;
  origin: string;
}

export interface MockOptions {
  /**
   * "blocked" (default): every Kiwi Builds check ends needing the Mac. "simulated": a build runs
   * through the Mac's stages and publishes a signed release in the Kiwi Builds source.
   */
  builds?: "blocked" | "simulated";
  /**
   * The device the mock answers as: an iPhone (default), an Android phone, or a desktop browser
   * (nothing tells its platform, so iOS is assumed until one is chosen).
   */
  device?: Platform | "desktop";
  /** false: the device has not been through the first-run setup yet. Default true. */
  onboarded?: boolean;
  /** The program asking: the web app (default) or Kiwi's Android app. */
  client?: Client;
  /** The computer Kiwi runs on: a Mac (default) or Linux. */
  host?: "macos" | "linux";
  /**
   * Whether Kiwi serves its Android app (default true). "failed": signing it failed, so Kiwi
   * serves none and warns Android devices (`android_app`) as the server does.
   */
  androidApp?: boolean | "failed";
  /** Android packages installed with another signature than Kiwi's, e.g. "app.morphe.android.youtube". */
  foreign?: string[];
  /**
   * How the imitated Kiwi signs: "keychain" (a Mac) or "files" (Linux: uploaded identity and
   * profiles, which the upload endpoints change). Defaults to the host's.
   */
  signing?: SigningBackend;
}

/** The certificate the mock's Android builds are signed with (`PUT /api/me/packages`). */
export const MOCK_ANDROID_SIGNER = "c0ffee".padEnd(64, "0");

const MOCK_ANDROID_APP = {
  package: "io.github.digitalpals.kiwi",
  version_name: "1.0.0",
  version_code: 1,
  size: 9_400_000,
};

/** Mirrors kiwi-server's `routes::features`: the Kiwi app is offered only to devices detected as Android. */
export function featuresOf(platform: Platform, detected: Platform | null, client: Client, host: Host, androidApp: boolean): Feature[] {
  const web = client === "web";
  const ios = platform === "ios";
  const features: [boolean, Feature][] = [
    [ios, "ios_apps"],
    [!ios, "android_apps"],
    [web && ios, "home_screen_app"],
    [web, "web_push"],
    [web && !ios && detected === "android" && androidApp, "android_app_offer"],
    [ios && host.os === "macos", "devicectl"],
  ];
  return features.filter(([on]) => on).map(([, feature]) => feature);
}

export interface MockResponse {
  status: number;
  json?: unknown;
  text?: string;
  contentType?: string;
  headers?: Record<string, string>;
}

const VERSION = "0.2.0";
const INSTALL_WINDOW = 10 * 60_000;
const ORPHAN_SOURCE_NAME = "Removed source";
/** The built-in Kiwi Builds source. */
const BUILD_SOURCE = "kiwi-youtube";
/** How long each stage of a simulated Kiwi Build takes; the web app polls every 2 seconds. */
const BUILD_STAGE_MS = 2600;
const SETUP_CHECKS = ["Mac build tools", "Tweak toolchain", "App Store sign-in", "Private decryption enrollment"];
/** kiwi-server's setup checks where iOS apps can't be built (`automation::NEEDS_MAC_CHECK`). */
const NEEDS_MAC_CHECKS: SetupCheck[] = [{ name: "macOS", ready: false }];
/** The message of kiwi-server's `409 needs_mac`. */
const NEEDS_MAC = "iOS apps can only be built on a Mac. This Kiwi doesn't run on macOS, so Kiwi Builds builds Android apps here.";
/** Why the mock's Kiwi couldn't sign its Android app (`KIWI_MOCK_ANDROID_APP=failed`). */
const ANDROID_APP_FAILURE = "The Android builder failed: apksigner was not found in the pinned Android build-tools.";

class HttpError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

const iso = (ms: number) => new Date(ms).toISOString();
const appKey = (app: { source_id: string; bundle_id: string }) => `${app.source_id}/${app.bundle_id}`;
const bundleOf = (key: string) => key.split(/\/(.*)/)[1] ?? key;

export function compareVersions(a: string, b: string): number {
  const pa = a.split(/[.-]/).map((p) => Number.parseInt(p, 10) || 0);
  const pb = b.split(/[.-]/).map((p) => Number.parseInt(p, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return Math.sign(d);
  }
  return 0;
}

/** Mirrors kiwi-server's `store::short_name`: "Spotify: Music and Podcasts" is "Spotify". */
export function shortName(name: string): string {
  const trimmed = name.trim();
  const cuts = [": ", " - ", " – ", " — ", " | "].map((sep) => trimmed.indexOf(sep)).filter((i) => i >= 0);
  if (cuts.length === 0) return trimmed;
  const short = trimmed.slice(0, Math.min(...cuts)).trim();
  return short.length >= 2 ? short : trimmed;
}

function vapidPublicKey(): string {
  const { publicKey } = generateKeyPairSync("ec", { namedCurve: "prime256v1" });
  const jwk = publicKey.export({ format: "jwk" });
  const raw = Buffer.concat([Buffer.from([4]), Buffer.from(jwk.x ?? "", "base64url"), Buffer.from(jwk.y ?? "", "base64url")]);
  return raw.toString("base64url");
}

/** A Kiwi Builds release is newer than the installed copy: a newer version or a later revision of it. */
function newerThanInstalled(target: Release, installed: Installed): boolean {
  const c = compareVersions(target.version, installed.version);
  return c > 0 || (c === 0 && (target.revision ?? 0) > (installed.revision ?? 0));
}

export function createMockApi(clock: () => number = Date.now, options: MockOptions = {}) {
  const db: MockDb = createDb(
    clock(),
    options.device === "android" ? "android" : "ios",
    options.signing ?? (options.host === "linux" ? "files" : "keychain"),
  );
  if (options.device === "desktop") {
    Object.assign(db.device, {
      id: "local:mQ3x7Lr2Vt9KpZ4wYb8N1c",
      name: "Browser",
      kind: "browser",
      tailscale_name: null,
      os_version: null,
      os_version_source: null,
      model: null,
    });
    db.detected_platform = null;
    db.installs = [];
    db.attempts = [];
  }
  db.onboarded = options.onboarded ?? true;
  db.foreign_packages = new Set(options.foreign ?? []);
  const client: Client = options.client ?? "web";
  const host: Host = {
    ...(options.host === "linux"
      ? { os: "linux", ios_signing: "files", devicectl: false }
      : { os: "macos", ios_signing: "keychain", devicectl: true }),
    android_signer_sha256: MOCK_ANDROID_SIGNER,
  };
  const servesAndroidApp = (options.androidApp ?? true) === true;
  const androidAppFailed = options.androidApp === "failed";
  /** iOS apps are built only on a Mac (`409 needs_mac` elsewhere). */
  const buildsIos = host.os === "macos";
  const vapid = vapidPublicKey();
  const builds = createBuilds(iso(clock()));
  const simulated = options.builds === "simulated";

  // ---- derived state ------------------------------------------------------------------

  /** The device's platform: the owner's choice, else the detected one, else iOS. */
  function syncPlatform() {
    const chosen = db.chosen_platform;
    db.device.platform = chosen ?? db.detected_platform ?? "ios";
    db.device.platform_source = chosen ? "chosen" : db.detected_platform ? "detected" : "assumed";
    db.device.detected_platform = db.detected_platform;
  }
  syncPlatform();

  const deviceJson = (now: number): Device => ({ ...db.device, client, last_seen: iso(now) });

  const androidApp = (origin: string): AndroidApp | null =>
    servesAndroidApp ? { ...MOCK_ANDROID_APP, install_url: `${origin}/android/kiwi.apk` } : null;

  const platformOf = (app: Pick<MockApp, "platform">): Platform => app.platform ?? "ios";
  const onDevice = (app: MockApp) => platformOf(app) === db.device.platform;
  const isAndroidKey = (key: string) => db.apps.find((a) => appKey(a) === key)?.platform === "android";

  function release(app: MockApp, r: MockRelease): Release {
    const os = db.device.os_version;
    let incompatibility: string | null = null;
    if (!onDevice(app)) incompatibility = platformOf(app) === "android" ? "Made for Android devices" : "Made for iPhone and iPad";
    else if (os && r.min_os && compareVersions(os, r.min_os) < 0) incompatibility = `Requires iOS ${r.min_os} or later`;
    else if (os && r.max_os && compareVersions(os, r.max_os) > 0) incompatibility = `Requires iOS ${r.max_os} or earlier`;
    return { ...r, compatible: incompatibility === null, incompatibility };
  }

  const releasesOf = (app: MockApp): Release[] => app.releases.map((r) => release(app, r));

  function sourceName(sourceId: string): string {
    return db.sources.find((s) => s.id === sourceId)?.name ?? ORPHAN_SOURCE_NAME;
  }

  // ---- Browse choices and App Store originals -------------------------------------------

  function isShown(app: MockApp): boolean {
    const source = db.sources.find((s) => s.id === app.source_id);
    return app.shown ?? source?.browse !== "picked";
  }

  function isNew(app: MockApp): boolean {
    const source = db.sources.find((s) => s.id === app.source_id);
    return source?.browse === "picked" && app.shown === undefined && !!app.is_new;
  }

  function sourceJson(source: MockSource): Source {
    if (source.id === BUILD_SOURCE) {
      const apps = db.apps.filter((a) => a.source_id === source.id);
      return { ...source, kind: "local_build", ipa_url: null, app_count: apps.length, shown_count: apps.filter(isShown).length, new_count: 0 };
    }
    const apps = db.apps.filter((a) => a.source_id === source.id);
    // Sources without fixture apps keep their stated count, as if every app were shown.
    if (apps.length === 0) return { ...source, kind: "feed", ipa_url: null, shown_count: source.browse === "all" ? source.app_count : 0, new_count: 0 };
    return { ...source, kind: "feed", ipa_url: null, app_count: apps.length, shown_count: apps.filter(isShown).length, new_count: apps.filter(isNew).length };
  }

  function storeApps(): MockStoreApp[] {
    return db.store[db.settings.app_store_country] ?? [];
  }

  function storeJson(a: MockStoreApp): StoreApp {
    const { slug, tint: _tint, ...rest } = a;
    return { ...rest, short_name: shortName(a.name), icon_url: `/__mock/icon/${slug}.svg` };
  }

  function findStore(bundleId: string): MockStoreApp | undefined {
    return storeApps().find((a) => a.bundle_id.toLowerCase() === bundleId.toLowerCase());
  }

  function originalOf(app: MockApp): { match: OriginalMatch; store: MockStoreApp | null } {
    // An Android app has no counterpart in Apple's store.
    if (app.original === null || app.platform === "android") return { match: "none", store: null };
    const store = findStore(app.original ?? app.bundle_id) ?? null;
    if (!store) return { match: "not_found", store: null };
    return { match: app.original === undefined ? "bundle_id" : "linked", store };
  }

  function originalJson(app: MockApp, target: Release | null): Original | null {
    const { store } = originalOf(app);
    if (!store) return null;
    const appVersion = target?.version ?? app.releases[0]?.version ?? null;
    let status: Original["status"] = "unknown";
    if (appVersion && /^\d+(\.\d+)*$/.test(appVersion) && /^\d+(\.\d+)*$/.test(store.version)) {
      const c = compareVersions(appVersion, store.version);
      status = c === 0 ? "same" : c < 0 ? "behind" : "ahead";
    }
    return { ...storeJson(store), app_version: appVersion, status };
  }

  function sameApp(app: MockApp): string {
    return (originalOf(app).store?.bundle_id ?? app.bundle_id).toLowerCase();
  }

  function otherVersions(app: MockApp): OtherVersion[] {
    const key = appKey(app);
    return visibleApps()
      .filter((o) => appKey(o) !== key && sameApp(o) === sameApp(app))
      .map((o) => ({
        source_id: o.source_id,
        source_name: sourceName(o.source_id),
        bundle_id: o.bundle_id,
        name: o.name,
        icon_url: `/__mock/icon/${o.slug}.svg`,
        tint_color: o.tint_color,
        version: o.releases[0]?.version ?? null,
        installed: !!db.installs.find((i) => i.app_key === appKey(o)),
        hidden: !isShown(o),
      }));
  }

  function buildStatus(b: MockBuild, now: number): Pick<Build, "status" | "step" | "progress" | "error" | "finished_at"> {
    const t = (now - b.started) / b.duration;
    const finished = iso(b.started + b.duration);
    if (b.error && t >= 0.6) return { status: "failed", step: null, progress: null, error: b.error, finished_at: finished };
    if (t >= 1) return { status: "ready", step: null, progress: 1, error: null, finished_at: finished };
    if (t < 0.08) return { status: "queued", step: "Waiting for a download slot", progress: null, error: null, finished_at: null };
    if (t < 0.45) {
      const done = (b.size * (t - 0.08)) / 0.37 / 1e6;
      return {
        status: "downloading",
        step: `Downloading ${done.toFixed(1)} of ${(b.size / 1e6).toFixed(1)} MB`,
        progress: t,
        error: null,
        finished_at: null,
      };
    }
    if (t < 0.9) {
      const total = 23;
      const done = Math.min(total, Math.max(1, Math.round(((t - 0.45) / 0.45) * total)));
      return { status: "signing", step: `Signing ${done} of ${total}`, progress: t, error: null, finished_at: null };
    }
    return { status: "packaging", step: "Packaging", progress: t, error: null, finished_at: null };
  }

  /** An iPhone installs from a manifest; an Android phone downloads the APK itself. */
  function installUrl(token: string, origin: string, android: boolean): string {
    if (android) return `${origin}/ota/${token}/app.apk`;
    return `itms-services://?action=download-manifest&url=${encodeURIComponent(`${origin}/ota/${token}/manifest.plist`)}`;
  }

  function buildJson(b: MockBuild, now: number, origin: string): Build {
    const [sourceId = "", bundleId = ""] = b.app_key.split(/\/(.*)/);
    const status = buildStatus(b, now);
    const android = isAndroidKey(b.app_key);
    return {
      id: b.id,
      source_id: sourceId,
      bundle_id: bundleId,
      version: b.version,
      build: b.build,
      ...status,
      // An APK keeps its package name and never expires.
      signed_bundle_id: android ? bundleId : `${bundleId}.${TEAM_ID}`,
      size: b.size,
      expires_at: android ? null : iso(Math.min(PROFILE_EXPIRES, CERT_EXPIRES)),
      dropped_entitlements: b.dropped,
      created_at: iso(b.started),
      install_url: status.status === "ready" ? installUrl(b.token, origin, android) : null,
    };
  }

  function latestBuild(key: string, version: string, revision?: number): MockBuild | undefined {
    return db.builds.filter((b) => b.app_key === key && b.version === version && b.revision === revision).at(-1);
  }

  function activeAttempt(buildId: string, now: number): MockAttempt | undefined {
    return db.attempts.find(
      (a) => a.build_id === buildId && a.manifest_at !== null && !a.completed_at && now - a.manifest_at < INSTALL_WINDOW,
    );
  }

  function installed(key: string): Installed | null {
    const i = db.installs.find((x) => x.app_key === key);
    if (!i) return null;
    const bundleId = key.split(/\/(.*)/)[1] ?? key;
    return {
      version: i.version,
      build: i.build,
      ...(i.revision === undefined ? {} : { revision: i.revision }),
      signed_bundle_id: i.origin === "kiwi" && !isAndroidKey(key) ? `${bundleId}.${TEAM_ID}` : bundleId,
      origin: i.origin,
      verified: i.verified,
      installed_at: iso(i.installed_at),
      expires_at: i.expires_at === null ? null : iso(i.expires_at),
    };
  }

  /** A Kiwi install of another source's app under the signed id this app would use. */
  function replaces(app: MockApp): AppState["replaces"] {
    const key = appKey(app);
    const other = db.installs.find((i) => i.origin === "kiwi" && i.app_key !== key && bundleOf(i.app_key) === app.bundle_id);
    if (!other) return null;
    const otherApp = db.apps.find((a) => appKey(a) === other.app_key);
    return {
      name: otherApp?.name ?? bundleOf(other.app_key),
      source_name: sourceName(other.app_key.split("/")[0] ?? ""),
      version: other.version,
    };
  }

  function needsResign(key: string, now: number): boolean {
    const i = db.installs.find((x) => x.app_key === key);
    return !!i && i.origin === "kiwi" && i.expires_at !== null && i.expires_at - now < 30 * DAY;
  }

  function appState(app: MockApp, now: number, origin: string): AppState {
    const key = appKey(app);
    const inst = installed(key);
    const install = db.installs.find((x) => x.app_key === key);
    const openUrl = install?.kiwi_scheme
      ? `kiwi-${createHash("sha256").update(`${app.bundle_id}.${TEAM_ID}`).digest("hex").slice(0, 12)}://`
      : null;
    const releases = releasesOf(app);
    const target = releases.find((r) => r.compatible) ?? null;
    const base = {
      installed: inst,
      open_url: openUrl,
      update_reason: null,
      install_url: null,
      replaces: inst ? null : replaces(app),
      foreign_install: platformOf(app) === "android" && db.foreign_packages.has(app.bundle_id),
    };
    if (!target) return { ...base, action: inst ? "open" : "unavailable", target: null, build: null };

    const mockBuild = latestBuild(key, target.version, target.revision);
    const build = mockBuild ? buildJson(mockBuild, now, origin) : null;
    let action: AppState["action"];
    let reason: AppState["update_reason"] = null;
    if (build && build.status !== "ready" && build.status !== "failed") action = "preparing";
    else if (build?.status === "failed") action = "retry";
    else if (inst && newerThanInstalled(target, inst)) {
      action = "update";
      reason = "new_version";
    } else if (inst && needsResign(key, now)) {
      action = "update";
      reason = "resign";
    } else if (inst) action = "open";
    else if (build?.status === "ready") action = "install";
    else action = "get";

    if ((action === "install" || action === "update") && mockBuild && activeAttempt(mockBuild.id, now)) {
      action = "installing";
    }
    const ready = build?.status === "ready" && (action === "install" || action === "update" || action === "installing");
    return {
      ...base,
      action,
      update_reason: action === "update" || action === "installing" ? reason : null,
      target,
      build,
      install_url: ready ? build.install_url : null,
    };
  }

  function summary(app: MockApp, now: number, origin: string): AppSummary {
    const state = appState(app, now, origin);
    const newest = app.releases[0];
    return {
      source_id: app.source_id,
      source_name: sourceName(app.source_id),
      bundle_id: app.bundle_id,
      platform: platformOf(app),
      name: app.name,
      developer: app.developer,
      subtitle: app.subtitle,
      icon_url: `/__mock/icon/${app.slug}.svg`,
      tint_color: app.tint_color,
      category: app.category,
      unavailable: state.action === "unavailable" && newest ? release(app, newest).incompatibility : null,
      orphaned: !db.sources.some((s) => s.id === app.source_id),
      hidden: !isShown(app),
      is_new: isNew(app),
      original: originalJson(app, state.target),
      state,
    };
  }

  function detail(app: MockApp, now: number, origin: string): AppDetail {
    return {
      ...summary(app, now, origin),
      description: app.description,
      screenshots: Array.from({ length: app.screenshots }, (_, i) => `/__mock/shot/${app.slug}-${i + 1}.svg`),
      releases: releasesOf(app),
      permissions: app.permissions,
      dropped_entitlements: app.dropped_entitlements,
      signed_bundle_id: app.platform === "android" ? app.bundle_id : `${app.bundle_id}.${TEAM_ID}`,
      source_url: db.sources.find((s) => s.id === app.source_id)?.url ?? "",
      original_match: originalOf(app).match,
      other_versions: otherVersions(app),
    };
  }

  function isUpdate(app: MockApp, now: number): boolean {
    const key = appKey(app);
    const inst = installed(key);
    if (!inst) return false;
    const target = releasesOf(app).find((r) => r.compatible);
    return (!!target && newerThanInstalled(target, inst)) || needsResign(key, now);
  }

  const visibleApps = () => db.apps.filter((a) => db.sources.some((s) => s.id === a.source_id && s.enabled));

  /** Why Kiwi can't sign, in the server's words; `no_identity` when there is no identity at all. */
  function signingProblem(): { code: "no_identity" | "no_profile"; message: string } | null {
    const files = db.signing_backend === "files";
    if (!db.identities[0]) {
      return {
        code: "no_identity",
        message: files
          ? "No signing identity was uploaded. Export your Apple Development certificate with its private key as a .p12 file and upload it."
          : "No code signing identity was found in the login keychain. Sign in to Xcode and create an Apple Development certificate.",
      };
    }
    if (!db.profiles.some((p) => p.usable && p.wildcard)) {
      return {
        code: "no_profile",
        message: files
          ? "No usable wildcard provisioning profile was found. Download a profile for the App ID * that includes this certificate from the Apple Developer site and upload it."
          : "No usable wildcard provisioning profile was found. Open an app project in Xcode with automatic signing once, then rescan.",
      };
    }
    return null;
  }

  function me(now: number, origin: string): Me {
    const updates = visibleApps().filter((a) => isUpdate(a, now)).length;
    const errors = db.sources.filter((s) => s.enabled && s.error);
    const warnings: Warning[] = [];
    // Signing doesn't concern Android devices.
    const problem = db.device.platform === "android" ? null : signingProblem();
    if (problem) warnings.push({ level: "error", code: problem.code, link: "/settings", message: problem.message });
    if (errors.length > 0) {
      warnings.push({
        level: "warning",
        code: "source_errors",
        link: "/sources",
        message: `${errors.length === 1 ? "1 source" : `${errors.length} sources`} failed to refresh. Check Sources for details.`,
      });
    }
    for (const install of db.installs) {
      const app = db.apps.find((a) => appKey(a) === install.app_key);
      if (app && install.expires_at !== null && install.expires_at - now < 7 * DAY) {
        const days = Math.max(0, Math.floor((install.expires_at - now) / DAY));
        warnings.push({
          level: "warning",
          code: "expiring",
          link: "/apps",
          message: `${app.name} expires in ${days} ${days === 1 ? "day" : "days"}. Update it to re-sign.`,
        });
      }
    }
    // Signing Kiwi's own Android app failed: the server tells Android devices only.
    if (androidAppFailed && db.device.platform === "android") {
      warnings.push({
        level: "warning",
        code: "android_app",
        link: "/settings",
        message: `Kiwi couldn't prepare its Android app: ${ANDROID_APP_FAILURE}`,
      });
    }
    const offer = androidApp(origin);
    return {
      device: deviceJson(now),
      onboarded: db.onboarded,
      server: {
        version: VERSION,
        public_url: "https://mac-studio.example.ts.net",
        team_id: TEAM_ID,
        devicectl_available: host.devicectl,
      },
      host,
      features: featuresOf(db.device.platform, db.detected_platform, client, host, offer !== null),
      android_app: offer,
      push: { vapid_public_key: vapid, subscribed: db.subscriptions.size > 0 },
      counts: {
        updates,
        installed: db.installs.length,
        sources: db.sources.length,
        source_errors: errors.length,
        preparing: db.builds.filter((b) => {
          const s = buildStatus(b, now).status;
          return s !== "ready" && s !== "failed";
        }).length,
      },
      warnings,
    };
  }

  function devices(now: number): DevicesResponse {
    return { devices: [deviceJson(now)], core_devices: db.core_devices };
  }

  /** Completes simulated installs and builds whose time has come. */
  function tick(now: number) {
    advanceBuilds(now);
    for (const attempt of db.attempts) {
      if (attempt.manifest_at === null || attempt.completed_at || now - attempt.manifest_at < attempt.install_ms) continue;
      attempt.completed_at = now;
      const build = db.builds.find((b) => b.id === attempt.build_id);
      if (!build) continue;
      recordInstall(build, now);
      db.device.os_version_source = db.device.os_version_source === "manual" ? "manual" : "installer";
    }
  }

  /** The device has the build: an IPA signed until the profile expires, or an APK that doesn't expire. */
  function recordInstall(build: MockBuild, now: number) {
    const android = isAndroidKey(build.app_key);
    // Kiwi installs share the signed bundle id of every source's app with this bundle id.
    db.installs = db.installs.filter(
      (i) => i.app_key !== build.app_key && !(i.origin === "kiwi" && bundleOf(i.app_key) === bundleOf(build.app_key)),
    );
    db.installs.push({
      app_key: build.app_key,
      version: build.version,
      build: build.build,
      ...(build.revision === undefined ? {} : { revision: build.revision }),
      origin: "kiwi",
      verified: false,
      installed_at: now,
      expires_at: android ? null : Math.min(PROFILE_EXPIRES, CERT_EXPIRES),
      kiwi_scheme: !android,
    });
  }

  // ---- Kiwi Builds ----------------------------------------------------------------------

  /** App Store setup checks, reported once a simulated App Store build checks them. */
  let setupChecks: SetupCheck[] = [];
  /** When each simulated job got the Mac's single build worker, and the jobs Rebuild forced. */
  const jobStarts = new Map<string, number>();
  const forcedJobs = new Set<string>();
  let workerFreeAt = 0;

  /** The App Store version a build uses: the one its tweak supports, or the storefront's newest. */
  const buildVersion = (app: AutomationState) => app.app_version ?? findStore(app.bundle_id)?.version ?? "1.0";

  const recipeKey = (app: AutomationState, version: string, p: AutomationPreferences) =>
    createHash("sha256").update(JSON.stringify([app.id, version, [...p.tweaks].sort(), p.name ?? null])).digest("hex");

  /** An app's state as the server reports it, with what follows from the published build. */
  function automationView(app: AutomationState): AutomationState {
    const view = structuredClone(app);
    const published = app.published;
    view.configuration_changed = !!published && (
      [...(published.tweaks ?? [])].sort().join(",") !== [...app.preferences.tweaks].sort().join(",")
      || (app.names.length > 0 && published.name !== null && published.name !== (app.preferences.name ?? null)));
    view.source_enabled = db.sources.find((s) => s.id === BUILD_SOURCE)?.enabled ?? null;
    // The server shows the App Store icon.
    const store = simulated ? findStore(iconBundleId(app.id)) : undefined;
    if (store) view.icon_url = `/__mock/icon/${store.slug}.svg`;
    // Where iOS apps can't be built, only the missing Mac is checked and nothing is scheduled.
    if (app.platform !== "android" && !buildsIos) Object.assign(view, { checks: currentChecks(), next_check_at: null });
    return view;
  }

  /** The shared setup checks: the App Store's, or only the missing Mac where iOS apps can't be built. */
  const currentChecks = (): SetupCheck[] => structuredClone(buildsIos ? setupChecks : NEEDS_MAC_CHECKS);

  /** Simulated builds: the Mac's one worker takes the oldest job through its stages, then publishes it. */
  function advanceBuilds(now: number) {
    if (!simulated) return;
    for (;;) {
      const queue = builds
        .flatMap((app) => app.jobs.filter((j) => !j.finished_at).map((job) => ({ app, job })))
        .sort((a, b) => a.job.created_at.localeCompare(b.job.created_at));
      const next = queue[0];
      if (!next) return;
      const { app, job } = next;
      const start = jobStarts.get(job.id) ?? Math.max(Date.parse(job.created_at), workerFreeAt);
      jobStarts.set(job.id, start);
      const stages = buildStages(app);
      const step = Math.floor((now - start) / BUILD_STAGE_MS);
      // Android builds need no Mac setup: no Apple account, no decryption.
      if (step >= 1 && setupChecks.length === 0 && app.platform !== "android") {
        setupChecks = SETUP_CHECKS.map((name) => ({ name, ready: true }));
        for (const b of builds) if (b.platform !== "android") b.checks = structuredClone(setupChecks);
      }
      if (step >= 2 && job.recipe_key === null) {
        const version = buildVersion(app);
        const key = recipeKey(app, version, job.preferences ?? app.preferences);
        Object.assign(job, { version, external_id: String(880_000_000 + (Number.parseInt(key.slice(0, 6), 16) % 9_000_000)), recipe_key: key });
      }
      if (step >= 2 && !forcedJobs.has(job.id) && job.recipe_key === app.last_success_key) {
        finishJob(app, job, start + 2 * BUILD_STAGE_MS, "unchanged");
        continue;
      }
      if (step < stages.length) {
        job.status = "running";
        job.stage = stages[step] ?? "";
        for (const waiting of queue.slice(1)) {
          waiting.job.status = "queued";
          waiting.job.stage = `Waiting for ${app.name}`;
        }
        return;
      }
      finishJob(app, job, start + stages.length * BUILD_STAGE_MS, "ready");
    }
  }

  function finishJob(app: AutomationState, job: AutomationJob, at: number, status: "ready" | "unchanged" | "cancelled") {
    workerFreeAt = Math.max(workerFreeAt, at);
    job.status = status;
    job.finished_at = iso(at);
    if (status === "unchanged") job.stage = "Already up to date";
    else if (status === "cancelled") {
      job.stage = "Cancelled";
      job.error = "The build was cancelled. The previous release is still available.";
    } else {
      job.stage = app.platform === "android" ? "Available in Kiwi" : "Signed and available in Kiwi";
      publishBuild(app, job, at);
    }
  }

  const BUILD_SIZES: Record<string, number> = {
    youtube: 152.6, "youtube-music": 98.4, twitch: 121.7, x: 164.2, reddit: 139.5,
    "android-youtube": 187.3, "android-youtube-music": 81.9, "android-reddit": 96.2,
  };

  /** Signs a finished build and lists it in the Kiwi Builds source, like the Mac does. */
  function publishBuild(app: AutomationState, job: AutomationJob, at: number) {
    const preferences = job.preferences ?? app.preferences;
    const version = job.version ?? buildVersion(app);
    const catalog = catalogOf(app.id, preferences.name);
    let source = db.sources.find((s) => s.id === BUILD_SOURCE);
    if (!source) {
      source = {
        id: BUILD_SOURCE, url: "https://kiwi.invalid/automation/builds", name: "Kiwi Builds", subtitle: "Built on your Mac",
        icon_url: null, enabled: true, browse: "all", app_count: 0, added_at: iso(at), last_attempt_at: null,
        last_success_at: null, error: null, diagnostics: [],
      };
      db.sources.push(source);
    }
    source.last_attempt_at = source.last_success_at = iso(at);
    let entry = db.apps.find((a) => a.source_id === BUILD_SOURCE && a.bundle_id === app.bundle_id);
    const android = app.platform === "android";
    if (!entry) {
      entry = {
        source_id: BUILD_SOURCE, bundle_id: app.bundle_id, slug: findStore(iconBundleId(app.id))?.slug ?? `kiwi-${app.id}`,
        name: catalog.name, developer: null, subtitle: null, tint_color: null, category: null, description: null,
        screenshots: 0, releases: [],
        permissions: { entitlements: android ? [] : ["application-identifier", "keychain-access-groups", "aps-environment"], privacy: [] },
        dropped_entitlements: android ? [] : ["aps-environment"],
        ...(android ? { platform: "android" as const } : {}),
      };
      db.apps.push(entry);
    }
    Object.assign(entry, {
      name: catalog.name, developer: catalog.developer, subtitle: catalog.subtitle, tint_color: catalog.tint_color,
      category: catalog.category, description: catalog.description,
    });
    const size = Math.round((BUILD_SIZES[app.id] ?? 100) * 1e6);
    const recipe = `Recipe ${(job.recipe_key ?? "").slice(0, 12)}. Not yet tested on a physical device.`;
    const notes = android
      ? `${app.name} ${version} · Android build ${job.external_id}\nOriginal from Google Play, verified against the publisher’s signing certificate.\n`
        + `Patched with ${app.tweak}; signed with this Kiwi’s own key.\n${recipe}`
      : `${app.name} ${version} · Apple build ${job.external_id}\n${app.tweak.split(" ")[0]} and tweaks compiled from pinned sources.\n${recipe}`;
    entry.releases = [
      { version, build: version, revision: job.sequence, date: iso(at), notes, size, min_os: android ? null : "15.0", max_os: null },
      ...entry.releases,
    ].slice(0, catalog.retain);
    const signed: MockBuild = {
      id: `b-${randomBytes(6).toString("hex")}`, app_key: appKey(entry), version, build: version, revision: job.sequence,
      started: at - 2000, duration: 2000, error: null, size, dropped: entry.dropped_entitlements,
      token: `t-${randomBytes(9).toString("base64url")}`,
    };
    db.builds.push(signed);
    job.signed_build_id = signed.id;
    Object.assign(app, {
      published: { version, build: version, revision: job.sequence, date: iso(at), tweaks: [...preferences.tweaks], name: preferences.name ?? null },
      last_success_key: job.recipe_key,
      last_success_tweaks: [...preferences.tweaks],
      last_success_name: preferences.name ?? null,
      failed_recipe: null,
      failed_attempts: 0,
    });
  }

  function cancelBuild(app: AutomationState, now: number) {
    const job = app.jobs.find((j) => !j.finished_at);
    if (job) finishJob(app, job, now, "cancelled");
  }

  // ---- request handling ---------------------------------------------------------------

  function findApp(sourceId: string, bundleId: string): MockApp {
    const app = db.apps.find((a) => a.source_id === sourceId && a.bundle_id === bundleId);
    if (!app || !db.sources.some((s) => s.id === sourceId)) throw new HttpError(404, "not_found", "App not found.");
    return app;
  }

  function findSource(id: string): MockSource {
    const source = db.sources.find((s) => s.id === id);
    if (!source) throw new HttpError(404, "not_found", "Source not found.");
    return source;
  }

  function prepare(app: MockApp, body: { version?: unknown; build?: unknown; revision?: unknown }, now: number): MockBuild {
    const releases = releasesOf(app);
    const wanted =
      typeof body.version === "string"
        ? releases.find((r) => r.version === body.version && (typeof body.build !== "string" || r.build === body.build)
          && (body.revision === undefined || r.revision === body.revision))
        : releases.find((r) => r.compatible);
    if (!wanted) throw new HttpError(400, "bad_request", "No matching release to prepare.");
    if (!wanted.compatible) throw new HttpError(400, "bad_request", wanted.incompatibility ?? "Incompatible release.");
    const key = appKey(app);
    const existing = latestBuild(key, wanted.version, wanted.revision);
    if (existing && buildStatus(existing, now).status !== "failed") return existing;
    const build: MockBuild = {
      id: `b-${randomBytes(6).toString("hex")}`,
      app_key: key,
      version: wanted.version,
      build: wanted.build,
      ...(wanted.revision === undefined ? {} : { revision: wanted.revision }),
      started: now,
      duration: 9000 + Math.round(Math.random() * 4000),
      error: null,
      size: Math.round((wanted.size ?? 50e6) * 0.996),
      dropped: app.dropped_entitlements,
      token: `t-${randomBytes(9).toString("base64url")}`,
    };
    db.builds.push(build);
    return build;
  }

  function parseBody(req: MockRequest): Record<string, unknown> {
    if (!req.body) return {};
    try {
      const value: unknown = JSON.parse(req.body);
      if (value && typeof value === "object" && !Array.isArray(value)) return value as Record<string, unknown>;
    } catch {
      // fall through
    }
    throw new HttpError(400, "bad_request", "Request body must be a JSON object.");
  }

  function route(req: MockRequest, now: number): MockResponse {
    const { method, path, origin } = req;
    const parts = path.split("/").filter(Boolean).map(decodeURIComponent);
    const ok = (json: unknown, status = 200): MockResponse => ({ status, json });

    if (parts[0] === "android") {
      if (parts[1] !== "kiwi.apk" || !servesAndroidApp) throw new HttpError(404, "not_found", "Kiwi's Android app is not available here.");
      return {
        status: 200,
        contentType: "application/vnd.android.package-archive",
        text: "PK mock Kiwi app",
        headers: { "Content-Disposition": `attachment; filename="Kiwi-${MOCK_ANDROID_APP.version_name}.apk"` },
      };
    }
    if (parts[0] === "ota") {
      const attempt = db.attempts.find((a) => a.token === parts[1]) ?? null;
      const build = db.builds.find((b) => b.token === parts[1]);
      if (!build) throw new HttpError(404, "not_found", "Unknown install token.");
      const android = isAndroidKey(build.app_key);
      if (parts[2] === "app.apk" && android) {
        // Downloading the whole APK counts as the install; Android shows nothing more to Kiwi.
        recordInstall(build, now);
        return {
          status: 200,
          contentType: "application/vnd.android.package-archive",
          text: "PK mock",
          headers: { "Content-Disposition": `attachment; filename="${bundleOf(build.app_key)}-${build.version}.apk"` },
        };
      }
      if (parts[2] === "manifest.plist" && !android) {
        if (attempt) attempt.manifest_at = now;
        else {
          db.attempts.push({
            token: build.token,
            build_id: build.id,
            manifest_at: now,
            install_ms: 6000,
            completed_at: null,
          });
        }
        return {
          status: 200,
          contentType: "application/xml",
          text: `<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>items</key><array><dict><key>metadata</key><dict><key>bundle-identifier</key><string>${build.app_key.split(/\/(.*)/)[1]}.${TEAM_ID}</string><key>bundle-version</key><string>${build.version}</string><key>kind</key><string>software</string><key>title</key><string>Mock</string></dict></dict></array></dict></plist>\n`,
        };
      }
      throw new HttpError(404, "not_found", "Not available in mock mode.");
    }

    if (method !== "GET" && method !== "HEAD") {
      if (req.headers["x-kiwi"] !== "1") throw new HttpError(403, "csrf", "Missing X-Kiwi header.");
      if (req.body && !(req.headers["content-type"] ?? "").includes("application/json")) {
        throw new HttpError(403, "csrf", "Content-Type must be application/json.");
      }
    }
    const body = parseBody(req);
    const [, resource, id, sub] = parts;
    const is = (m: string, ...segments: (string | undefined)[]) =>
      method === m && segments.length === parts.length - 1 && segments.every((s, i) => s === undefined || s === parts[i + 1]);

    if (is("GET", "health")) return ok({ ok: true, version: VERSION });
    if (resource === "automation" && !db.settings.kiwi_builds) {
      throw new HttpError(409, "builds_off", "Kiwi Builds is off. Turn it on in Settings.");
    }
    if (is("GET", "automation")) return ok({ apps: builds.map(automationView), checks: currentChecks() });
    if (parts[1] === "automation" && parts[2] !== undefined) {
      const build = builds.find((b) => b.id === parts[2]);
      if (!build) throw new HttpError(404, "not_found", "Kiwi doesn't build that app.");
      const preferences = (): AutomationPreferences => {
        const { enabled, interval_minutes, tweaks, name } = body;
        const known = build.available_tweaks.map((t) => t.id);
        if (typeof enabled !== "boolean" || typeof interval_minutes !== "number" || !Number.isInteger(interval_minutes)
          || interval_minutes < 15 || interval_minutes > 1440 || !Array.isArray(tweaks)
          || tweaks.length > known.length || tweaks.some((t) => typeof t !== "string" || !known.includes(t))
          || (name !== undefined && !build.names.includes(name as string))
          || Object.keys(body).some((k) => !["enabled", "interval_minutes", "tweaks", "name"].includes(k))) {
          throw new HttpError(400, "bad_request", "Invalid automatic build preferences.");
        }
        const chosen = typeof name === "string" ? name : build.names[0];
        return { enabled, interval_minutes, tweaks: [...new Set(tweaks as string[])].sort(), ...(chosen ? { name: chosen } : {}) };
      };
      // Like the server: an iOS app can't be added or checked where it can't be built.
      const requireBuildable = () => {
        if (build.platform !== "android" && !buildsIos) throw new HttpError(409, "needs_mac", NEEDS_MAC);
      };
      const schedule = () => {
        build.next_check_at = build.added && build.preferences.enabled
          ? iso(now + build.preferences.interval_minutes * 60_000) : null;
      };
      // Without simulated builds the mock cannot run the Mac pipeline: every check ends blocked.
      const check = (force = false) => {
        requireBuildable();
        if (!build.added) throw new HttpError(400, "bad_request", `Add ${build.name} to Kiwi Builds first.`);
        if (build.jobs.some((j) => !j.finished_at)) throw new HttpError(409, "conflict", `A ${build.name} build job is already running.`);
        build.sequence++;
        build.last_checked_at = iso(now);
        if (simulated) {
          const running = builds.find((b) => b.jobs.some((j) => !j.finished_at));
          build.jobs.unshift({
            id: `mock-${build.id}-${build.sequence}`, app: build.id, sequence: build.sequence, status: "queued",
            stage: running ? `Waiting for ${running.name}` : "Checking build setup", version: null, external_id: null,
            recipe_key: null, signed_build_id: null, error: null, created_at: iso(now), finished_at: null,
            preferences: structuredClone(build.preferences),
          });
          build.jobs = build.jobs.slice(0, 30);
          if (force) forcedJobs.add(`mock-${build.id}-${build.sequence}`);
          schedule();
          advanceBuilds(now);
          return;
        }
        if (build.platform !== "android") build.checks = [{ name: "Mac build tools", ready: false }];
        build.jobs.unshift({
          id: `mock-${build.id}-${build.sequence}`, app: build.id, sequence: build.sequence, status: "blocked",
          stage: "Needs attention", version: null, external_id: null, recipe_key: null, signed_build_id: null,
          error: "Mock mode cannot build apps. Connect to Kiwi on your Mac to run the pipeline.",
          created_at: iso(now), finished_at: iso(now), preferences: structuredClone(build.preferences),
        });
        build.jobs = build.jobs.slice(0, 30);
        schedule();
      };
      if (is("GET", "automation", undefined)) return ok(automationView(build));
      if (is("PATCH", "automation", undefined)) {
        build.preferences = preferences();
        schedule();
        return ok(automationView(build));
      }
      if (is("POST", "automation", undefined, "add")) {
        requireBuildable();
        build.preferences = preferences();
        build.added = true;
        check();
        return ok(automationView(build), 202);
      }
      if (is("DELETE", "automation", undefined)) {
        cancelBuild(build, now);
        build.added = false;
        build.published = null;
        // Installed copies keep working; the source no longer offers the app.
        db.apps = db.apps.filter((a) => !(a.source_id === BUILD_SOURCE && a.bundle_id === build.bundle_id));
        schedule();
        return ok(automationView(build));
      }
      if (is("POST", "automation", undefined, "check")) {
        check(body.force === true);
        return ok(automationView(build), 202);
      }
      if (is("POST", "automation", undefined, "cancel")) {
        cancelBuild(build, now);
        return ok(automationView(build));
      }
    }
    if (is("GET", "me")) return ok(me(now, origin));
    if (is("PUT", "me", "packages")) {
      if (db.device.platform !== "android") throw new HttpError(400, "bad_request", "Only Android devices report their installed packages.");
      if (!Array.isArray(body.packages)) throw new HttpError(400, "bad_request", "Invalid request body: missing field `packages`.");
      const android = new Set(db.apps.filter((a) => a.platform === "android").map((a) => a.bundle_id));
      db.foreign_packages = new Set(
        (body.packages as { package?: unknown; signer_sha256?: unknown }[])
          .filter((p) => typeof p.package === "string" && android.has(p.package))
          .filter((p) => !(Array.isArray(p.signer_sha256) && p.signer_sha256.includes(MOCK_ANDROID_SIGNER)))
          .map((p) => p.package as string),
      );
      return { status: 204 };
    }
    if (is("PATCH", "me")) {
      if (body.platform !== undefined) {
        if (body.platform !== null && body.platform !== "ios" && body.platform !== "android") {
          throw new HttpError(400, "bad_request", "Invalid request body: unknown variant, expected `ios` or `android`.");
        }
        db.chosen_platform = body.platform;
        syncPlatform();
      }
      if (body.onboarded !== undefined) {
        if (typeof body.onboarded !== "boolean") throw new HttpError(400, "bad_request", "Invalid request body: onboarded must be true or false.");
        db.onboarded = body.onboarded;
      }
      if (body.name !== undefined) {
        if (typeof body.name !== "string" || !body.name.trim()) throw new HttpError(400, "bad_request", "Name can't be empty.");
        db.device.name = body.name.trim();
      }
      if (body.os_version === null) {
        db.device.os_version = db.installer_os;
        db.device.os_version_source = "installer";
      } else if (body.os_version !== undefined) {
        if (typeof body.os_version !== "string" || !/^\d+(\.\d+){0,2}$/.test(body.os_version.trim())) {
          throw new HttpError(400, "bad_request", "Use a version like 27.0.1.");
        }
        db.device.os_version = body.os_version.trim();
        db.device.os_version_source = "manual";
      }
      return ok(me(now, origin));
    }

    if (resource === "sources") {
      if (is("GET", "sources")) return ok(db.sources.map(sourceJson));
      if (is("POST", "sources", "refresh")) {
        for (const s of db.sources.filter((x) => x.enabled)) refreshSource(s, now);
        return ok(db.sources.map(sourceJson));
      }
      if (is("POST", "sources")) return ok(sourceJson(addSource(body.url, now)), 201);
      const source = findSource(id ?? "");
      if (is("PATCH", "sources", id)) {
        if (body.enabled !== undefined && typeof body.enabled !== "boolean") {
          throw new HttpError(400, "bad_request", "enabled must be a boolean.");
        }
        if (body.browse !== undefined && body.browse !== "all" && body.browse !== "picked") {
          throw new HttpError(400, "bad_request", 'browse must be "all" or "picked".');
        }
        if (typeof body.enabled === "boolean") source.enabled = body.enabled;
        if (body.browse && body.browse !== source.browse) {
          source.browse = body.browse;
          // Nothing is new right after switching.
          for (const a of db.apps) if (a.source_id === source.id) a.is_new = false;
        }
        return ok(sourceJson(source));
      }
      if (is("DELETE", "sources", id)) {
        db.sources = db.sources.filter((s) => s !== source);
        for (const a of db.apps) if (a.source_id === source.id) a.shown = undefined;
        return { status: 204 };
      }
      if (is("POST", "sources", id, "refresh")) return ok(sourceJson(refreshSource(source, now)));
    }

    if (is("GET", "store", "search")) {
      const term = (req.query.get("term") ?? "").trim().toLowerCase();
      if (!term) throw new HttpError(400, "bad_request", "Enter an app name to search for.");
      return ok(storeApps().filter((a) => a.name.toLowerCase().includes(term)).map(storeJson));
    }

    if (resource === "apps") {
      if (is("GET", "apps")) {
        const q = (req.query.get("q") ?? "").trim().toLowerCase();
        const sourceFilter = req.query.get("source");
        const withHidden = ["1", "true"].includes(req.query.get("hidden") ?? "");
        return ok(
          visibleApps()
            .filter((a) => !sourceFilter || a.source_id === sourceFilter)
            // Browse leaves out the other platform's apps; a source's own page lists them all.
            .filter((a) => withHidden || (isShown(a) && onDevice(a)))
            .filter((a) => !q || [a.name, a.developer, a.subtitle, a.bundle_id].some((f) => f?.toLowerCase().includes(q)))
            .sort((a, b) => a.name.localeCompare(b.name))
            .map((a) => summary(a, now, origin)),
        );
      }
      const app = findApp(id ?? "", sub ?? "");
      if (is("GET", "apps", id, sub)) return ok(detail(app, now, origin));
      if (is("PATCH", "apps", id, sub)) {
        if (body.hidden !== undefined && typeof body.hidden !== "boolean") {
          throw new HttpError(400, "bad_request", "hidden must be true or false.");
        }
        if (body.original !== undefined) {
          const original = body.original as { bundle_id?: unknown } | null;
          if (original === null) app.original = undefined;
          else if (original.bundle_id === null) app.original = null;
          else if (typeof original.bundle_id === "string") {
            const store = findStore(original.bundle_id);
            if (!store) {
              throw new HttpError(
                400,
                "bad_request",
                `No app in the App Store (${db.settings.app_store_country.toUpperCase()}) has the bundle ID ${original.bundle_id}.`,
              );
            }
            app.original = store.bundle_id;
          } else throw new HttpError(400, "bad_request", "Invalid original app.");
        }
        if (typeof body.hidden === "boolean") {
          app.shown = !body.hidden;
          app.is_new = false;
        }
        return ok(detail(app, now, origin));
      }
      if (is("POST", "apps", id, sub, "prepare")) return ok(buildJson(prepare(app, body, now), now, origin), 202);
    }

    if (is("GET", "builds", id)) {
      const build = db.builds.find((b) => b.id === id);
      if (!build) throw new HttpError(404, "not_found", "Build not found.");
      return ok(buildJson(build, now, origin));
    }

    if (is("GET", "library")) {
      return ok(
        db.installs
          .map((i) => db.apps.find((a) => appKey(a) === i.app_key))
          .filter((a): a is MockApp => !!a)
          .sort((a, b) => a.name.localeCompare(b.name))
          .map((a) => summary(a, now, origin)),
      );
    }
    if (is("DELETE", "library", id)) {
      const before = db.installs.length;
      db.installs = db.installs.filter((i) => installed(i.app_key)?.signed_bundle_id !== id);
      if (db.installs.length === before) throw new HttpError(404, "not_found", "Not in the library.");
      return { status: 204 };
    }

    if (is("GET", "updates")) {
      return ok(visibleApps().filter((a) => isUpdate(a, now)).map((a) => summary(a, now, origin)));
    }
    if (is("POST", "updates", "prepare")) {
      return ok(
        visibleApps()
          .filter((a) => isUpdate(a, now))
          .map((a) => buildJson(prepare(a, {}, now), now, origin)),
      );
    }

    const signing = (): Signing => {
      const identity = db.identities[0];
      const profile = db.profiles.find((p) => p.usable && p.wildcard);
      return {
        backend: db.signing_backend,
        identities: db.identities,
        profiles: db.profiles,
        active:
          identity && profile
            ? { identity_sha1: identity.sha1, profile_uuid: profile.uuid, expires_at: iso(Math.min(PROFILE_EXPIRES, CERT_EXPIRES)) }
            : null,
        error: signingProblem()?.message ?? null,
      };
    };
    const base64 = (field: string, value: unknown): Buffer => {
      if (typeof value !== "string" || !value.trim() || !/^[A-Za-z0-9+/=\s]+$/.test(value)) {
        throw new HttpError(400, "bad_request", `${field} must be base64.`);
      }
      return Buffer.from(value, "base64");
    };
    if (is("GET", "signing") || is("POST", "signing", "rescan")) return ok(signing());
    if (is("POST", "signing", "identity") || is("DELETE", "signing", "identity")) {
      if (db.signing_backend !== "files") {
        throw new HttpError(
          409,
          "conflict",
          "This Kiwi signs with the Mac's login keychain. Add the certificate to the keychain (for example in Xcode) instead of uploading it.",
        );
      }
      if (method === "DELETE") {
        if (!db.identities.length) throw new HttpError(404, "not_found", "No signing identity was uploaded.");
        db.identities = [];
        return ok(signing());
      }
      base64("p12", body.p12);
      // The mock can't open a .p12; the password "wrong" imitates a bad one.
      if (body.password === "wrong") throw new HttpError(400, "bad_request", "The password is wrong");
      db.identities = [
        {
          sha1: createHash("sha1").update(String(body.p12)).digest("hex").toUpperCase(),
          name: `Apple Development: Alex Morgan (${TEAM_ID})`,
          team_id: TEAM_ID,
          expires_at: iso(CERT_EXPIRES),
        },
      ];
      return ok(signing());
    }
    if (is("POST", "signing", "profiles")) {
      const bytes = base64("profile", body.profile);
      // The mock can't parse a profile: it adds a usable team wildcard profile per upload.
      const uuid = createHash("sha256").update(bytes).digest("hex").slice(0, 32).toUpperCase()
        .replace(/^(.{8})(.{4})(.{4})(.{4})/, "$1-$2-$3-$4-");
      db.profiles = db.profiles.filter((p) => p.uuid !== uuid);
      db.profiles.unshift({
        uuid,
        name: "iOS Team Provisioning Profile: *",
        team_id: TEAM_ID,
        app_id_pattern: `${TEAM_ID}.*`,
        wildcard: true,
        development: true,
        created_at: iso(clock()),
        expires_at: iso(PROFILE_EXPIRES),
        devices: 2,
        entitlements: ["application-identifier", "com.apple.developer.team-identifier", "get-task-allow", "keychain-access-groups"],
        usable: db.identities.length > 0,
        reason: db.identities.length > 0 ? null : "No signing identity",
        uploaded: true,
      });
      return ok(signing());
    }
    if (is("DELETE", "signing", "profiles", undefined)) {
      const before = db.profiles.length;
      db.profiles = db.profiles.filter((p) => !(p.uploaded && p.uuid === sub));
      if (db.profiles.length === before) {
        throw new HttpError(404, "not_found", "No uploaded provisioning profile has this UUID. Profiles from Xcode can't be removed here.");
      }
      return ok(signing());
    }

    if (is("GET", "settings")) return ok(db.settings);
    if (is("PATCH", "settings")) {
      const next = { ...db.settings } as Record<string, unknown>;
      for (const [k, v] of Object.entries(body)) {
        if (!(k in db.settings)) throw new HttpError(400, "bad_request", `Unknown setting ${k}.`);
        if (typeof v !== typeof next[k]) throw new HttpError(400, "bad_request", `Invalid value for ${k}.`);
        next[k] = v;
      }
      const settings = next as unknown as Settings;
      settings.app_store_country = settings.app_store_country.trim().toLowerCase();
      if (!/^[a-z]{2}$/.test(settings.app_store_country)) {
        throw new HttpError(400, "bad_request", 'app_store_country must be a two-letter country code such as "nl"');
      }
      if (settings.refresh_interval_minutes < 15) throw new HttpError(400, "bad_request", "Refresh interval must be at least 15 minutes.");
      if (settings.devicectl_sync_minutes < 5) throw new HttpError(400, "bad_request", "Sync interval must be at least 5 minutes.");
      // Turning Kiwi Builds off cancels its queued and running builds.
      if (!settings.kiwi_builds) for (const b of builds) cancelBuild(b, now);
      db.settings = settings;
      return ok(db.settings);
    }

    if (is("POST", "push", "subscribe")) {
      const keys = body.keys as { p256dh?: unknown; auth?: unknown } | undefined;
      if (typeof body.endpoint !== "string" || typeof keys?.p256dh !== "string" || typeof keys.auth !== "string") {
        throw new HttpError(400, "bad_request", "Invalid subscription.");
      }
      db.subscriptions.add(body.endpoint);
      return { status: 204 };
    }
    if (is("POST", "push", "unsubscribe")) {
      if (typeof body.endpoint === "string") db.subscriptions.delete(body.endpoint);
      return { status: 204 };
    }
    if (is("POST", "push", "test")) return ok({ sent: db.subscriptions.size });

    if (is("GET", "devices")) return ok(devices(now));
    if (is("POST", "devices", "sync")) {
      const linked = db.device.devicectl;
      if (linked) {
        linked.last_sync_at = iso(now);
        linked.last_error = linked.reachable ? null : "The device is not connected (tunnel unavailable).";
      }
      return ok(devices(now));
    }
    if (is("PATCH", "devices", id)) {
      if (id !== db.device.id) throw new HttpError(404, "not_found", "Device not found.");
      if (typeof body.name === "string" && body.name.trim()) db.device.name = body.name.trim();
      if (body.devicectl_id !== undefined) {
        for (const c of db.core_devices) if (c.linked_device_id === db.device.id) c.linked_device_id = null;
        if (body.devicectl_id === null) db.device.devicectl = null;
        else {
          const core = db.core_devices.find((c) => c.id === body.devicectl_id);
          if (!core) throw new HttpError(404, "not_found", "No such device on the Mac.");
          core.linked_device_id = db.device.id;
          db.device.devicectl = {
            id: core.id,
            name: core.name,
            reachable: core.pairing_state === "paired" && core.tunnel_state === "connected",
            last_sync_at: null,
            last_error: null,
          };
        }
      }
      return ok(deviceJson(now));
    }

    throw new HttpError(404, "not_found", `No route for ${method} ${path}.`);
  }

  function refreshSource(source: MockSource, now: number): MockSource {
    source.last_attempt_at = iso(now);
    if (!source.error) source.last_success_at = iso(now);
    return source;
  }

  function addSource(url: unknown, now: number): MockSource {
    if (typeof url !== "string" || !url.trim()) throw new HttpError(400, "bad_request", "Enter a source URL.");
    let parsed: URL;
    try {
      parsed = new URL(url.trim());
    } catch {
      throw new HttpError(400, "bad_request", "That doesn't look like a URL.");
    }
    if (parsed.protocol !== "https:") throw new HttpError(400, "bad_request", "Source URLs must use https.");
    if (db.sources.some((s) => new URL(s.url).href === parsed.href)) throw new HttpError(409, "conflict", "This source is already added.");
    if (parsed.hostname.includes("fail") || parsed.hostname.endsWith(".invalid")) {
      throw new HttpError(502, "source_fetch", `Couldn't fetch the source: HTTP 404 Not Found from ${parsed.hostname}.`);
    }
    if (!parsed.pathname.endsWith(".json") && parsed.pathname !== "/") {
      throw new HttpError(422, "source_parse", 'The response is not an AltStore source (expected an object with an "apps" array).');
    }
    const source: MockSource = {
      id: createHash("sha256").update(parsed.href).digest("hex").slice(0, 16),
      url: parsed.href,
      name: parsed.hostname.replace(/^www\./, ""),
      subtitle: null,
      icon_url: null,
      enabled: true,
      browse: "all",
      app_count: 0,
      added_at: iso(now),
      last_attempt_at: iso(now),
      last_success_at: iso(now),
      error: null,
      diagnostics: ["The source lists no apps"],
    };
    db.sources.push(source);
    return source;
  }

  return {
    db,
    handle(req: MockRequest): MockResponse {
      const now = clock();
      tick(now);
      try {
        return route(req, now);
      } catch (error) {
        if (error instanceof HttpError) return { status: error.status, json: { error: { code: error.code, message: error.message } } };
        return { status: 500, json: { error: { code: "internal", message: String(error) } } };
      }
    },
    /** A new version of an app on the App Store, so Kiwi Builds has something to update to. */
    releaseOnAppStore(bundleId: string, version: string): boolean {
      const listings = Object.values(db.store).flat().filter((a) => a.bundle_id.toLowerCase() === bundleId.toLowerCase());
      for (const listing of listings) Object.assign(listing, { version, released_at: iso(clock()) });
      return listings.length > 0;
    },
    /** Looks up an app for the generated placeholder artwork. */
    artwork(slug: string): { name: string; tint: string } | null {
      const app = db.apps.find((a) => a.slug === slug);
      const source = db.sources.find((s) => s.id === slug);
      const store = Object.values(db.store)
        .flat()
        .find((a) => a.slug === slug);
      if (app) return { name: app.name, tint: app.tint_color ?? "#888888" };
      if (source) return { name: source.name, tint: "#5856D6" };
      if (store) return { name: store.name, tint: store.tint };
      return null;
    },
  };
}

export type MockApi = ReturnType<typeof createMockApi>;
