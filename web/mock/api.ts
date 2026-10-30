// In-memory imitation of kiwi-server for `VITE_MOCK=1 npm run dev`. Follows docs/api.md and the
// action rules in docs/architecture.md closely enough to exercise every UI state.
import { createHash, generateKeyPairSync, randomBytes } from "node:crypto";
import type {
  AppAccess,
  AppDetail,
  AppleAccount,
  AppState,
  AppSummary,
  AutomationPreferences,
  Build,
  DevicesResponse,
  Family,
  FamilyDevice,
  Installed,
  Me,
  Original,
  OriginalMatch,
  OtherVersion,
  Person,
  PersonApp,
  PersonDetail,
  Registration,
  Release,
  Settings,
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
  type MockPerson,
  type MockViewer,
  type MockBuild,
  type MockDb,
  type MockRelease,
  type MockSource,
  type MockStoreApp,
} from "./fixtures.ts";
import { createBuilds } from "./builds.ts";

export interface MockRequest {
  method: string;
  path: string;
  query: URLSearchParams;
  headers: Record<string, string | undefined>;
  body: string;
  origin: string;
}

export interface MockResponse {
  status: number;
  json?: unknown;
  text?: string;
  contentType?: string;
  headers?: Record<string, string>;
}

/** The cookie that picks whose device the dev app is (`?as=emma`); the owner's by default. */
export const VIEWER_COOKIE = "kiwi_mock_as";

export function viewerFromCookie(cookie: string | undefined): string {
  const match = /(?:^|;\s*)kiwi_mock_as=([a-z]+)/.exec(cookie ?? "");
  return match?.[1] ?? "alex";
}

const VERSION = "0.2.0";
const INSTALL_WINDOW = 10 * 60_000;
const ORPHAN_SOURCE_NAME = "Removed source";

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

export function createMockApi(clock: () => number = Date.now) {
  const db: MockDb = createDb(clock());
  const vapid = vapidPublicKey();
  const builds = createBuilds(iso(clock()));

  // ---- the family: whose device is asking, and what they may use ------------------------

  const family = db.family;
  let current: MockViewer = family.viewers[0] as MockViewer;

  function findViewer(key: string): MockViewer | undefined {
    return family.viewers.find((v) => v.key === key);
  }

  /** Makes a viewer's device, installs and subscriptions the ones `db` (and every helper) uses. */
  function select(key: string) {
    current.device = db.device;
    current.installs = db.installs;
    current.subscriptions = db.subscriptions;
    current = findViewer(key) ?? (family.viewers[0] as MockViewer);
    db.device = current.device;
    db.installs = current.installs;
    db.subscriptions = current.subscriptions;
  }

  /** A viewer's installs, also for the current one (whose list lives in `db`). */
  const installsOf = (v: MockViewer) => (v === current ? db.installs : v.installs);
  const isOwnerViewer = (v: MockViewer) => v.person_id === family.owner;
  const isOwner = () => isOwnerViewer(current);

  function personOf(id: string): MockPerson {
    let person = family.people.find((p) => p.id === id);
    if (!person) {
      person = { id, name: id.split("@")[0] ?? id, new_apps: false, choices: {} };
      family.people.push(person);
    }
    return person;
  }

  const ownerName = () => {
    const name = family.people.find((p) => p.id === family.owner)?.name ?? "Alex";
    return name.split(/\s+/)[0] ?? name;
  };

  function allowedFor(v: MockViewer, app: MockApp): boolean {
    if (isOwnerViewer(v)) return true;
    return v.registration.ready && personOf(v.person_id).choices[appKey(app)] === true;
  }
  const allowed = (app: MockApp) => allowedFor(current, app);

  function requireOwner() {
    if (!isOwner()) throw new HttpError(403, "forbidden", `Only ${ownerName()} can do this.`);
  }

  function registrationJson(v: MockViewer, ownerView: boolean): Registration {
    const r = v.registration;
    return { ...r, ready: isOwnerViewer(v) || r.ready, error: ownerView ? r.error : null };
  }

  /** Puts approved devices in the profile (registration with Apple, or a rescan). */
  function addToProfile(v: MockViewer) {
    v.ready_at = undefined;
    Object.assign(v.registration, { apple_registered: true, in_profile: true, ready: true, error: null });
  }

  /** Registration with Apple takes a few seconds once App Store Connect is connected. */
  function settle(now: number) {
    for (const v of family.viewers) {
      if (v.ready_at !== undefined && v.ready_at <= now && v.registration.status === "approved") {
        addToProfile(v);
        family.kiwi_profile ??= `Kiwi * ${iso(now).slice(0, 16).replace("T", " ").replace(":", ".")}`;
      }
    }
  }

  function appleJson(): AppleAccount {
    const base = db.profiles[0];
    const members = family.viewers.filter((v) => !isOwnerViewer(v) && v.registration.in_profile).length;
    return {
      ...family.apple,
      profile: base
        ? {
            uuid: base.uuid,
            name: family.kiwi_profile ?? base.name,
            devices: base.devices + members,
            expires_at: base.expires_at,
            kiwi: !!family.kiwi_profile,
          }
        : null,
    };
  }

  function familyDevice(v: MockViewer): FamilyDevice {
    const device = v === current ? db.device : v.device;
    return {
      id: device.id,
      name: device.name,
      person_id: v.person_id,
      model: v.registration.product ?? device.model,
      os_version: device.os_version,
      last_seen: device.last_seen,
      registration: registrationJson(v, true),
    };
  }

  const viewersOf = (person: MockPerson) => family.viewers.filter((v) => v.person_id === person.id);

  function personApps(person: MockPerson): PersonApp[] {
    const owner = person.id === family.owner;
    const installed = new Map<string, string>();
    for (const v of viewersOf(person)) for (const i of installsOf(v)) installed.set(i.app_key, i.version);
    return visibleApps()
      .map((app) => {
        const key = appKey(app);
        return {
          source_id: app.source_id,
          source_name: sourceName(app.source_id),
          bundle_id: app.bundle_id,
          name: app.name,
          subtitle: app.subtitle,
          icon_url: `/__mock/icon/${app.slug}.svg`,
          tint_color: app.tint_color,
          allowed: owner || person.choices[key] === true,
          explicit: !owner && key in person.choices,
          installed_version: installed.get(key) ?? null,
        };
      })
      .sort((a, b) => a.name.localeCompare(b.name));
  }

  function personJson(person: MockPerson): Person {
    const devices = viewersOf(person).map(familyDevice);
    return {
      id: person.id,
      name: person.name,
      role: person.id === family.owner ? "owner" : "member",
      new_apps: person.new_apps,
      app_count: personApps(person).filter((a) => a.allowed).length,
      devices,
      last_seen: devices.map((d) => d.last_seen).sort().at(-1) ?? iso(clock()),
    };
  }

  function personDetail(id: string): PersonDetail {
    const person = family.people.find((p) => p.id === id);
    if (!person) throw new HttpError(404, "not_found", "Person not found.");
    return { ...personJson(person), apps: personApps(person) };
  }

  /** The owner first, then members with a device, by name. */
  function people(): MockPerson[] {
    return family.people
      .filter((p) => p.id === family.owner || viewersOf(p).length > 0)
      .sort((a, b) => Number(b.id === family.owner) - Number(a.id === family.owner) || a.name.localeCompare(b.name));
  }

  function familyJson(): Family {
    return {
      people: people().map(personJson),
      pending: family.viewers.filter((v) => v.registration.status === "pending").length,
      apple: appleJson(),
    };
  }

  function appAccess(app: MockApp): AppAccess[] {
    const key = appKey(app);
    return people().map((person) => {
      const viewers = viewersOf(person);
      const install = viewers.flatMap(installsOf).find((i) => i.app_key === key);
      const owner = person.id === family.owner;
      return {
        person_id: person.id,
        name: person.name,
        role: owner ? "owner" : "member",
        allowed: owner || person.choices[key] === true,
        installed_version: install?.version ?? null,
        pending: !owner && !viewers.some((v) => v.registration.ready),
      };
    });
  }

  function deviceViewer(id: string): MockViewer {
    const v = family.viewers.find((x) => (x === current ? db.device : x.device).id === id);
    if (!v) throw new HttpError(404, "not_found", "Device not found.");
    return v;
  }

  // ---- derived state ------------------------------------------------------------------

  function release(r: MockRelease): Release {
    const os = db.device.os_version;
    let incompatibility: string | null = null;
    if (os && r.min_os && compareVersions(os, r.min_os) < 0) incompatibility = `Requires iOS ${r.min_os} or later`;
    if (os && r.max_os && compareVersions(os, r.max_os) > 0) incompatibility = `Requires iOS ${r.max_os} or earlier`;
    return { ...r, compatible: incompatibility === null, incompatibility };
  }

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
    const apps = db.apps.filter((a) => a.source_id === source.id);
    // Sources without fixture apps keep their stated count, as if every app were shown.
    if (apps.length === 0) return { ...source, kind: "feed", shown_count: source.browse === "all" ? source.app_count : 0, new_count: 0 };
    return { ...source, kind: "feed", app_count: apps.length, shown_count: apps.filter(isShown).length, new_count: apps.filter(isNew).length };
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
    if (app.original === null) return { match: "none", store: null };
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
      .filter((o) => appKey(o) !== key && sameApp(o) === sameApp(app) && allowed(o))
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

  function installUrl(token: string, origin: string): string {
    return `itms-services://?action=download-manifest&url=${encodeURIComponent(`${origin}/ota/${token}/manifest.plist`)}`;
  }

  function buildJson(b: MockBuild, now: number, origin: string): Build {
    const [sourceId = "", bundleId = ""] = b.app_key.split(/\/(.*)/);
    const status = buildStatus(b, now);
    return {
      id: b.id,
      source_id: sourceId,
      bundle_id: bundleId,
      version: b.version,
      build: b.build,
      ...status,
      signed_bundle_id: `${bundleId}.${TEAM_ID}`,
      size: b.size,
      expires_at: iso(Math.min(PROFILE_EXPIRES, CERT_EXPIRES)),
      dropped_entitlements: b.dropped,
      created_at: iso(b.started),
      install_url: status.status === "ready" ? installUrl(b.token, origin) : null,
    };
  }

  function latestBuild(key: string, version: string): MockBuild | undefined {
    return db.builds.filter((b) => b.app_key === key && b.version === version).at(-1);
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
      signed_bundle_id: i.origin === "kiwi" ? `${bundleId}.${TEAM_ID}` : bundleId,
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
    const releases = app.releases.map(release);
    const target = releases.find((r) => r.compatible) ?? null;
    const base = {
      installed: inst,
      open_url: openUrl,
      update_reason: null,
      install_url: null,
      replaces: inst ? null : replaces(app),
    };
    if (!target) return { ...base, action: inst ? "open" : "unavailable", target: null, build: null };

    const mockBuild = latestBuild(key, target.version);
    const build = mockBuild ? buildJson(mockBuild, now, origin) : null;
    let action: AppState["action"];
    let reason: AppState["update_reason"] = null;
    if (build && build.status !== "ready" && build.status !== "failed") action = "preparing";
    else if (build?.status === "failed") action = "retry";
    else if (inst && compareVersions(target.version, inst.version) > 0) {
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
      name: app.name,
      developer: app.developer,
      subtitle: app.subtitle,
      icon_url: `/__mock/icon/${app.slug}.svg`,
      tint_color: app.tint_color,
      category: app.category,
      unavailable: state.action === "unavailable" && newest ? release(newest).incompatibility : null,
      orphaned: !db.sources.some((s) => s.id === app.source_id),
      // The owner's Browse choices don't hide apps from members.
      hidden: isOwner() && !isShown(app),
      is_new: isOwner() && isNew(app),
      original: originalJson(app, state.target),
      state,
    };
  }

  function detail(app: MockApp, now: number, origin: string): AppDetail {
    return {
      ...summary(app, now, origin),
      description: app.description,
      screenshots: Array.from({ length: app.screenshots }, (_, i) => `/__mock/shot/${app.slug}-${i + 1}.svg`),
      releases: app.releases.map(release),
      permissions: app.permissions,
      dropped_entitlements: app.dropped_entitlements,
      signed_bundle_id: `${app.bundle_id}.${TEAM_ID}`,
      source_url: db.sources.find((s) => s.id === app.source_id)?.url ?? "",
      original_match: originalOf(app).match,
      other_versions: otherVersions(app),
      access: isOwner() ? appAccess(app) : null,
    };
  }

  function isUpdate(app: MockApp, now: number): boolean {
    const key = appKey(app);
    const inst = installed(key);
    if (!inst) return false;
    const target = app.releases.map(release).find((r) => r.compatible);
    return (!!target && compareVersions(target.version, inst.version) > 0) || needsResign(key, now);
  }

  const visibleApps = () => db.apps.filter((a) => db.sources.some((s) => s.id === a.source_id && s.enabled));

  function me(now: number): Me {
    const owner = isOwner();
    const updates = visibleApps().filter((a) => allowed(a) && isUpdate(a, now)).length;
    const errors = owner ? db.sources.filter((s) => s.enabled && s.error) : [];
    const warnings: Warning[] = [];
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
    return {
      device: { ...db.device, last_seen: iso(now) },
      server: {
        version: VERSION,
        public_url: "https://mac-studio.example.ts.net",
        team_id: TEAM_ID,
        devicectl_available: true,
      },
      push: { vapid_public_key: vapid, subscribed: db.subscriptions.size > 0 },
      counts: {
        updates,
        installed: db.installs.length,
        sources: owner ? db.sources.length : 0,
        source_errors: errors.length,
        preparing: db.builds.filter((b) => {
          const s = buildStatus(b, now).status;
          return s !== "ready" && s !== "failed";
        }).length,
      },
      warnings,
      access: {
        role: owner ? "owner" : "member",
        person: { id: current.person_id, name: personOf(current.person_id).name },
        owner_name: ownerName(),
      },
      registration: registrationJson(current, owner),
    };
  }

  function devices(): DevicesResponse {
    return { devices: [db.device], core_devices: db.core_devices };
  }

  /** Completes simulated installs whose time has come. */
  function tick(now: number) {
    settle(now);
    for (const attempt of db.attempts) {
      if (attempt.manifest_at === null || attempt.completed_at || now - attempt.manifest_at < attempt.install_ms) continue;
      attempt.completed_at = now;
      select(attempt.viewer ?? "alex");
      const build = db.builds.find((b) => b.id === attempt.build_id);
      if (!build) continue;
      // Kiwi installs share the signed bundle id of every source's app with this bundle id.
      db.installs = db.installs.filter(
        (i) => i.app_key !== build.app_key && !(i.origin === "kiwi" && bundleOf(i.app_key) === bundleOf(build.app_key)),
      );
      db.installs.push({
        app_key: build.app_key,
        version: build.version,
        build: build.build,
        origin: "kiwi",
        verified: false,
        installed_at: now,
        expires_at: Math.min(PROFILE_EXPIRES, CERT_EXPIRES),
        kiwi_scheme: true,
      });
      db.device.os_version_source = db.device.os_version_source === "manual" ? "manual" : "installer";
    }
    select("alex");
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
    const releases = app.releases.map(release);
    const wanted =
      typeof body.version === "string"
        ? releases.find((r) => r.version === body.version && (typeof body.build !== "string" || r.build === body.build)
          && (body.revision === undefined || r.revision === body.revision))
        : releases.find((r) => r.compatible);
    if (!wanted) throw new HttpError(400, "bad_request", "No matching release to prepare.");
    if (!wanted.compatible) throw new HttpError(400, "bad_request", wanted.incompatibility ?? "Incompatible release.");
    const key = appKey(app);
    const existing = latestBuild(key, wanted.version);
    if (existing && buildStatus(existing, now).status !== "failed") return existing;
    const build: MockBuild = {
      id: `b-${randomBytes(6).toString("hex")}`,
      app_key: key,
      version: wanted.version,
      build: wanted.build,
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

    if (parts[0] === "ota") {
      const attempt = db.attempts.find((a) => a.token === parts[1]) ?? null;
      const build = db.builds.find((b) => b.token === parts[1]);
      if (!build) throw new HttpError(404, "not_found", "Unknown install token.");
      if (parts[2] === "manifest.plist") {
        if (attempt) attempt.manifest_at = now;
        else {
          db.attempts.push({
            token: build.token,
            build_id: build.id,
            manifest_at: now,
            install_ms: 6000,
            completed_at: null,
            viewer: current.key,
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

    // The real server serves a profile; here downloading it stands for installing it in
    // Settings, after which iOS sends the UDID and Safari opens `/?enrolled=1`.
    if (parts[0] === "enroll") {
      const key = family.enrollments.get(parts[1] ?? "");
      const v = key ? findViewer(key) : undefined;
      if (!v) return { status: 404, text: "This registration link expired.", contentType: "text/plain" };
      family.enrollments.delete(parts[1] ?? "");
      const r = v.registration;
      const again = r.status === "approved";
      Object.assign(r, {
        status: isOwnerViewer(v) || again ? "approved" : "pending",
        udid: r.udid ?? `00008110-${randomBytes(8).toString("hex").toUpperCase()}`,
        product: r.product ?? v.device.model ?? "iPhone15,4",
        os_build: r.os_build ?? "24A434",
        requested_at: iso(now),
        decided_at: isOwnerViewer(v) ? iso(now) : again ? r.decided_at : null,
        error: null,
      });
      if (v.device.name === v.device.tailscale_name) v.device.name = `${personOf(v.person_id).name.split(/\s+/)[0]}'s iPhone`;
      return { status: 302, headers: { Location: "/?enrolled=1" } };
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
    const ownerOnly = ["automation", "sources", "store", "signing", "settings", "devices", "family"];
    if (resource && ownerOnly.includes(resource)) requireOwner();
    if (resource === "family") return familyRoute(req, now);
    if (is("POST", "me", "enroll")) {
      if (db.device.kind !== "tailscale") throw new HttpError(400, "bad_request", "Open Kiwi on the iPhone through Tailscale to register it.");
      const token = `e-${randomBytes(9).toString("base64url")}`;
      family.enrollments.set(token, current.key);
      return ok({ url: `/enroll/${token}/kiwi.mobileconfig` });
    }
    if (resource === "automation" && !db.settings.kiwi_builds) {
      throw new HttpError(409, "builds_off", "Kiwi Builds is off. Turn it on in Settings.");
    }
    if (is("GET", "automation")) return ok({ apps: structuredClone(builds), checks: [] });
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
      const schedule = () => {
        build.next_check_at = build.added && build.preferences.enabled
          ? iso(now + build.preferences.interval_minutes * 60_000) : null;
      };
      // Mock mode cannot run the Mac pipeline: every check ends blocked.
      const check = () => {
        if (!build.added) throw new HttpError(400, "bad_request", `Add ${build.name} to Kiwi Builds first.`);
        build.sequence++;
        build.last_checked_at = iso(now);
        build.checks = [{ name: "Mac build tools", ready: false }];
        build.jobs.unshift({
          id: `mock-${build.id}-${build.sequence}`, app: build.id, sequence: build.sequence, status: "blocked",
          stage: "Needs attention", version: null, external_id: null, recipe_key: null, signed_build_id: null,
          error: "Mock mode cannot build IPAs. Connect to Kiwi on your Mac to run the pipeline.",
          created_at: iso(now), finished_at: iso(now), preferences: structuredClone(build.preferences),
        });
        build.jobs = build.jobs.slice(0, 30);
        schedule();
      };
      if (is("GET", "automation", undefined)) return ok(structuredClone(build));
      if (is("PATCH", "automation", undefined)) {
        build.preferences = preferences();
        schedule();
        return ok(structuredClone(build));
      }
      if (is("POST", "automation", undefined, "add")) {
        build.preferences = preferences();
        build.added = true;
        check();
        return ok(structuredClone(build), 202);
      }
      if (is("DELETE", "automation", undefined)) {
        build.added = false;
        build.published = null;
        schedule();
        return ok(structuredClone(build));
      }
      if (is("POST", "automation", undefined, "check")) {
        check();
        return ok(structuredClone(build), 202);
      }
      if (is("POST", "automation", undefined, "cancel")) return ok(structuredClone(build));
    }
    if (is("GET", "me")) return ok(me(now));
    if (is("PATCH", "me")) {
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
      return ok(me(now));
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
            .filter((a) => allowed(a))
            .filter((a) => !sourceFilter || a.source_id === sourceFilter)
            .filter((a) => withHidden || !isOwner() || isShown(a))
            .filter((a) => !q || [a.name, a.developer, a.subtitle, a.bundle_id].some((f) => f?.toLowerCase().includes(q)))
            .sort((a, b) => a.name.localeCompare(b.name))
            .map((a) => summary(a, now, origin)),
        );
      }
      const app = findApp(id ?? "", sub ?? "");
      if (method === "POST" && !isOwner() && !current.registration.ready) {
        throw new HttpError(403, "not_registered", `This iPhone isn't registered yet. Register it in Kiwi; ${ownerName()} approves it.`);
      }
      if (!allowed(app)) throw new HttpError(404, "not_found", "App not found.");
      if (is("GET", "apps", id, sub)) return ok(detail(app, now, origin));
      if (is("PATCH", "apps", id, sub)) {
        requireOwner();
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
      const app = build && db.apps.find((a) => appKey(a) === build.app_key);
      if (!build || (!isOwner() && !(app && allowed(app)))) throw new HttpError(404, "not_found", "Build not found.");
      return ok(buildJson(build, now, origin));
    }

    if (is("GET", "library")) {
      return ok(
        db.installs
          .map((i) => db.apps.find((a) => appKey(a) === i.app_key))
          .filter((a): a is MockApp => !!a)
          .sort((a, b) => a.name.localeCompare(b.name))
          // Apps a member may no longer use: installed, never updated, no detail page.
          .map((a) => (allowed(a) ? summary(a, now, origin) : { ...summary(a, now, origin), orphaned: true })),
      );
    }
    if (is("DELETE", "library", id)) {
      const before = db.installs.length;
      db.installs = db.installs.filter((i) => installed(i.app_key)?.signed_bundle_id !== id);
      if (db.installs.length === before) throw new HttpError(404, "not_found", "Not in the library.");
      return { status: 204 };
    }

    if (is("GET", "updates")) {
      return ok(visibleApps().filter((a) => allowed(a) && isUpdate(a, now)).map((a) => summary(a, now, origin)));
    }
    if (is("POST", "updates", "prepare")) {
      return ok(
        visibleApps()
          .filter((a) => allowed(a) && isUpdate(a, now))
          .map((a) => buildJson(prepare(a, {}, now), now, origin)),
      );
    }

    if (is("POST", "signing", "rescan")) {
      // As if the owner had added approved iPhones to a profile in the Apple Developer portal.
      for (const v of family.viewers) if (v.registration.status === "approved" && !v.registration.in_profile) addToProfile(v);
    }
    if (is("GET", "signing") || is("POST", "signing", "rescan")) {
      const identity = db.identities[0];
      const profile = db.profiles[0];
      return ok({
        identities: db.identities,
        profiles: db.profiles,
        active:
          identity && profile
            ? { identity_sha1: identity.sha1, profile_uuid: profile.uuid, expires_at: iso(Math.min(PROFILE_EXPIRES, CERT_EXPIRES)) }
            : null,
        error: null,
      });
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

    if (is("GET", "devices")) return ok(devices());
    if (is("POST", "devices", "sync")) {
      const linked = db.device.devicectl;
      if (linked) {
        linked.last_sync_at = iso(now);
        linked.last_error = linked.reachable ? null : "The device is not connected (tunnel unavailable).";
      }
      return ok(devices());
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
      return ok(db.device);
    }

    throw new HttpError(404, "not_found", `No route for ${method} ${path}.`);
  }

  function familyRoute(req: MockRequest, now: number): MockResponse {
    const { method } = req;
    const parts = req.path.split("/").filter(Boolean).map(decodeURIComponent).slice(2);
    const body = parseBody(req);
    const ok = (json: unknown, status = 200): MockResponse => ({ status, json });
    const at = (m: string, ...segments: (string | null)[]) =>
      method === m && segments.length === parts.length && segments.every((s, i) => s === null || s === parts[i]);

    if (at("GET")) return ok(familyJson());
    if (at("GET", "apple")) return ok(appleJson());
    if (at("PUT", "apple")) {
      const issuer = typeof body.issuer_id === "string" ? body.issuer_id.trim().toLowerCase() : "";
      const keyId = typeof body.key_id === "string" ? body.key_id.trim().toUpperCase() : "";
      if (!/^[0-9a-f-]{36}$/.test(issuer)) throw new HttpError(400, "bad_request", "The issuer ID is a UUID like 57246542-96fe-1a63-e053-0824d011072a.");
      if (!/^[A-Z0-9]{8,16}$/.test(keyId)) throw new HttpError(400, "bad_request", "The key ID has 10 letters and digits, like 2X9R4HXF34.");
      if (typeof body.private_key !== "string" || !body.private_key.includes("PRIVATE KEY")) {
        throw new HttpError(400, "bad_request", "The private key is not an App Store Connect API key (.p8 file).");
      }
      Object.assign(family.apple, { connected: true, issuer_id: issuer, key_id: keyId, devices: 3, checked_at: iso(now), error: null });
      for (const v of family.viewers) if (v.registration.status === "approved" && !v.registration.in_profile) v.ready_at = now + 4000;
      return ok(appleJson());
    }
    if (at("DELETE", "apple")) {
      Object.assign(family.apple, { connected: false, issuer_id: null, key_id: null, devices: null, checked_at: null, error: null });
      return ok(appleJson());
    }
    if (at("POST", "apple", "sync")) {
      if (!family.apple.connected) throw new HttpError(400, "bad_request", "Connect an App Store Connect API key first.");
      family.apple.checked_at = iso(now);
      for (const v of family.viewers) if (v.registration.status === "approved" && !v.registration.in_profile) addToProfile(v);
      return ok(appleJson());
    }

    if (parts[0] === "people" && parts[1] !== undefined) {
      const id = parts[1];
      if (at("GET", "people", null)) return ok(personDetail(id));
      const person = family.people.find((p) => p.id === id);
      if (!person) throw new HttpError(404, "not_found", "Person not found.");
      const member = () => {
        if (id === family.owner) throw new HttpError(400, "bad_request", "The owner always has every app.");
      };
      if (at("PATCH", "people", null)) {
        if (typeof body.name === "string") {
          if (!body.name.trim()) throw new HttpError(400, "bad_request", "Names have 1 to 64 characters.");
          person.name = body.name.trim();
        }
        if (typeof body.new_apps === "boolean") {
          member();
          person.new_apps = body.new_apps;
        }
        return ok(personDetail(id));
      }
      if (at("PATCH", "people", null, "apps")) {
        member();
        const apps = body.apps;
        if (!Array.isArray(apps)) throw new HttpError(400, "bad_request", "apps must be a list.");
        for (const choice of apps as { source_id?: unknown; bundle_id?: unknown; allowed?: unknown }[]) {
          const key = `${String(choice.source_id)}/${String(choice.bundle_id)}`;
          if (!visibleApps().some((a) => appKey(a) === key) || typeof choice.allowed !== "boolean") {
            throw new HttpError(400, "bad_request", `${String(choice.bundle_id)} is not an app of an enabled source.`);
          }
          person.choices[key] = choice.allowed;
        }
        return ok(personDetail(id));
      }
      if (at("DELETE", "people", null)) {
        member();
        family.people = family.people.filter((p) => p !== person);
        for (const v of viewersOf(person)) {
          v.registration = { ...v.registration, status: "none", udid: null, requested_at: null, decided_at: null, apple_registered: false, in_profile: false, ready: false };
        }
        return { status: 204 };
      }
    }

    if (parts[0] === "devices" && parts[1] !== undefined) {
      const v = deviceViewer(parts[1]);
      const r = v.registration;
      if (at("GET", "devices", null)) return ok(familyDevice(v));
      const decide = (status: Registration["status"]) => {
        if (r.status === "none") throw new HttpError(404, "not_found", "This device has not sent its ID to Kiwi yet.");
        Object.assign(r, { status, decided_at: iso(now), error: null });
      };
      if (at("POST", "devices", null, "approve")) {
        decide("approved");
        if (family.apple.connected && !r.in_profile) v.ready_at = now + 4000;
        return ok(familyDevice(v));
      }
      if (at("POST", "devices", null, "decline")) {
        decide("declined");
        Object.assign(r, { in_profile: false, ready: false });
        return ok(familyDevice(v));
      }
      if (at("POST", "devices", null, "register")) {
        if (!family.apple.connected) {
          throw new HttpError(400, "bad_request", "Connect an App Store Connect API key first, or add the iPhone in the Apple Developer portal.");
        }
        if (r.status !== "approved") throw new HttpError(400, "bad_request", "Approve the device first.");
        v.ready_at = now + 3000;
        return ok(familyDevice(v), 202);
      }
      if (at("DELETE", "devices", null, "registration")) {
        v.registration = { ...r, status: "none", udid: null, requested_at: null, decided_at: null, apple_registered: false, in_profile: false, ready: false, error: null };
        return { status: 204 };
      }
    }
    throw new HttpError(404, "not_found", `No route for ${method} ${req.path}.`);
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
      select(viewerFromCookie(req.headers.cookie));
      try {
        return route(req, now);
      } catch (error) {
        if (error instanceof HttpError) return { status: error.status, json: { error: { code: error.code, message: error.message } } };
        return { status: 500, json: { error: { code: "internal", message: String(error) } } };
      } finally {
        select("alex");
      }
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
