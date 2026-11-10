// Mirrors docs/api.md exactly. Keep in sync with crates/kiwi-server.

export type Timestamp = string;

/** What a device installs: signed IPAs, or the APKs of Kiwi Builds' Android apps. */
export type Platform = "ios" | "android";

/** Where a device's platform comes from: the owner's choice, Tailscale or a User-Agent, or nothing (iOS is assumed). */
export type PlatformSource = "chosen" | "detected" | "assumed";

/** The program that sent a request: the web app (browser or Home Screen app) or Kiwi's Android app. */
export type Client = "web" | "android_app";

export interface Device {
  id: string; // "ts:<StableID>" | "local:<random>"
  name: string;
  kind: "tailscale" | "browser";
  /** What the device installs: the owner's choice, else the detected platform, else "ios". */
  platform: Platform;
  platform_source: PlatformSource;
  /** What Kiwi would take without a choice; null when nothing tells (desktop browsers). */
  detected_platform: Platform | null;
  /** This request's client; other devices (in `/api/devices`) are "web". */
  client: Client;
  tailscale_name: string | null;
  os_version: string | null;
  os_version_source: "devicectl" | "installer" | "manual" | "safari" | "android" | "android_app" | null;
  model: string | null;
  devicectl: LinkedCoreDevice | null;
  last_seen: Timestamp;
}

export interface LinkedCoreDevice {
  id: string;
  name: string;
  reachable: boolean;
  last_sync_at: Timestamp | null;
  last_error: string | null;
}

export interface CoreDevice {
  id: string;
  name: string;
  udid: string | null;
  model: string | null;
  platform: string | null;
  os_version: string | null;
  physical: boolean;
  pairing_state: string | null;
  tunnel_state: string | null;
  transport: string | null;
  linked_device_id: string | null;
}

export interface Warning {
  level: "info" | "warning" | "error";
  code: string;
  message: string;
  link: string | null;
}

/**
 * What to show on a device. `ios_apps` and `android_apps` follow the device's platform and
 * exclude each other.
 * - `ios_apps`: iOS installs, AltStore sources, apps from links, signing, expiry timers
 * - `android_apps`: Android installs (Kiwi Builds' APKs)
 * - `home_screen_app`: the web app on iOS: Add to Home Screen and Web Push
 * - `web_push`: the web app, which can use Web Push (iOS Home Screen app, Android and desktop browsers)
 * - `android_app_offer`: the web app on an Android device while Kiwi serves its Android app
 * - `devicectl`: an iOS device on a Mac: linking it to a paired CoreDevice
 */
export type Feature = "ios_apps" | "android_apps" | "home_screen_app" | "web_push" | "android_app_offer" | "devicectl";

/** The computer Kiwi runs on. */
export interface Host {
  os: "macos" | "linux" | "other";
  /** How this Kiwi signs iOS apps: the Mac's login keychain, or an uploaded identity file. */
  ios_signing: "keychain" | "files";
  devicectl: boolean;
  /** SHA-256 (hex) of the certificate of the key Kiwi signs Android apps with; null until Kiwi Builds has loaded it. */
  android_signer_sha256: string | null;
}

/** Kiwi's Android app, when this Kiwi serves it. */
export interface AndroidApp {
  package: string; // "io.github.digitalpals.kiwi"
  version_name: string;
  version_code: number;
  size: number;
  install_url: string; // "<public_url>/android/kiwi.apk"
}

export interface Me {
  device: Device;
  /** The first-run setup was completed on this device. */
  onboarded: boolean;
  server: {
    version: string;
    public_url: string;
    team_id: string | null;
    devicectl_available: boolean;
  };
  host: Host;
  features: Feature[];
  android_app: AndroidApp | null;
  push: { vapid_public_key: string; subscribed: boolean };
  counts: { updates: number; installed: number; sources: number; source_errors: number; preparing: number };
  warnings: Warning[];
}

/** One installed package, as Kiwi's Android app reports it (`PUT /api/me/packages`). */
export interface InstalledPackage {
  package: string;
  version_name: string | null;
  version_code: number;
  /** SHA-256 (hex) of each signing certificate. */
  signer_sha256: string[];
  installer: string | null;
}

/** `PUT /api/me/packages` (Kiwi's Android app): every user-installed package. */
export interface PackageReport {
  packages: InstalledPackage[];
}

export interface Source {
  id: string;
  /** "ipa": an IPA link or a download page; Kiwi reads the app from the IPA. */
  kind: "feed" | "ipa" | "local_build";
  /** The source's URL, an IPA's link, or the download page Kiwi follows. */
  url: string;
  /** The IPA an "ipa" source's app comes from now. */
  ipa_url: string | null;
  name: string;
  subtitle: string | null;
  icon_url: string | null;
  enabled: boolean;
  app_count: number;
  browse: BrowseMode;
  shown_count: number; // apps Browse shows
  new_count: number; // apps added since switching to "picked", not chosen yet
  added_at: Timestamp;
  last_attempt_at: Timestamp | null;
  last_success_at: Timestamp | null;
  error: string | null;
  diagnostics: string[];
}

/** "all": every app except the hidden ones; "picked": only the apps turned on. */
export type BrowseMode = "all" | "picked";

export interface Release {
  version: string;
  build: string | null;
  revision?: number;
  date: string | null;
  notes: string | null;
  size: number | null;
  min_os: string | null;
  max_os: string | null;
  compatible: boolean;
  incompatibility: string | null;
}

export type Action = "get" | "preparing" | "install" | "installing" | "open" | "update" | "retry" | "unavailable";

export interface Installed {
  version: string;
  build: string | null;
  revision?: number;
  signed_bundle_id: string;
  origin: "kiwi" | "device";
  verified: boolean;
  installed_at: Timestamp;
  expires_at: Timestamp | null;
}

/** Another source's app installed under the same signed bundle id; installing replaces it. */
export interface Replaces {
  name: string;
  source_name: string;
  version: string;
}

export interface AppState {
  action: Action;
  update_reason: "new_version" | "resign" | null;
  installed: Installed | null;
  target: Release | null;
  build: Build | null;
  install_url: string | null;
  open_url: string | null;
  replaces: Replaces | null;
  /** The app's package is installed with another signature (Android): remove that app first. */
  foreign_install: boolean;
}

/** An app in the App Store storefront chosen in Settings. */
export interface StoreApp {
  bundle_id: string;
  name: string; // "Spotify: Music and Podcasts"
  short_name: string; // "Spotify"
  developer: string | null;
  version: string;
  released_at: Timestamp | null; // of the current version
  icon_url: string | null;
  store_url: string; // apps.apple.com
}

/** "behind": the App Store has a newer version; "unknown": the versions can't be compared. */
export type VersionStatus = "same" | "behind" | "ahead" | "unknown";

/** The App Store app an app is based on (e.g. YouTube for a YouTube tweak). */
export interface Original extends StoreApp {
  app_version: string | null; // the signed IPA's version once built, else the release's
  status: VersionStatus;
}

export type OriginalMatch = "bundle_id" | "linked" | "not_found" | "none" | "pending";

export interface AppSummary {
  source_id: string;
  source_name: string;
  bundle_id: string;
  /** "android": an APK, installed as the package named by `bundle_id`. */
  platform: Platform;
  name: string;
  developer: string | null;
  subtitle: string | null;
  icon_url: string | null;
  tint_color: string | null;
  category: string | null;
  unavailable: string | null;
  orphaned: boolean;
  hidden: boolean; // not shown in Browse
  is_new: boolean; // added to a "picked" source since, not chosen yet
  original: Original | null;
  state: AppState;
}

/** Another source's version of the same app. */
export interface OtherVersion {
  source_id: string;
  source_name: string;
  bundle_id: string;
  name: string;
  icon_url: string | null;
  tint_color: string | null;
  version: string | null;
  installed: boolean;
  hidden: boolean;
}

export interface AppDetail extends AppSummary {
  description: string | null;
  screenshots: string[];
  releases: Release[];
  permissions: { entitlements: string[]; privacy: { key: string; description: string }[] };
  dropped_entitlements: string[];
  signed_bundle_id: string;
  source_url: string;
  original_match: OriginalMatch;
  other_versions: OtherVersion[];
}

export type BuildStatus = "queued" | "downloading" | "signing" | "packaging" | "ready" | "failed";

export interface Build {
  id: string;
  source_id: string;
  bundle_id: string;
  version: string;
  build: string | null;
  revision?: number;
  status: BuildStatus;
  step: string | null;
  progress: number | null;
  error: string | null;
  signed_bundle_id: string;
  size: number | null;
  expires_at: Timestamp | null;
  dropped_entitlements: string[];
  created_at: Timestamp;
  finished_at: Timestamp | null;
  install_url: string | null;
}

export interface Identity {
  sha1: string;
  name: string;
  team_id: string | null;
  expires_at: Timestamp | null;
}

export interface Profile {
  uuid: string;
  name: string;
  team_id: string;
  app_id_pattern: string;
  wildcard: boolean;
  development: boolean;
  created_at: Timestamp;
  expires_at: Timestamp;
  devices: number;
  entitlements: string[];
  usable: boolean;
  reason: string | null;
  /** Uploaded to Kiwi (DELETE /api/signing/profiles/:uuid removes it), not found in Xcode's folders. */
  uploaded: boolean;
}

/**
 * How this Kiwi signs iOS apps: "keychain" (macOS: the login keychain and codesign) or
 * "files" (other systems: an uploaded .p12 identity and rcodesign).
 */
export type SigningBackend = "keychain" | "files";

export interface Signing {
  backend: SigningBackend;
  identities: Identity[];
  profiles: Profile[];
  active: { identity_sha1: string; profile_uuid: string; expires_at: Timestamp } | null;
  error: string | null;
}

/** POST /api/signing/identity ("files" backend only; 409 on "keychain"). */
export interface IdentityUpload {
  /** The .p12 file, base64. */
  p12: string;
  password: string;
}

/** POST /api/signing/profiles. */
export interface ProfileUpload {
  /** The .mobileprovision file, base64. */
  profile: string;
}

export interface Settings {
  refresh_interval_minutes: number;
  auto_prepare_updates: boolean;
  notify_updates: boolean;
  notify_expiry: boolean;
  devicectl_sync: boolean;
  devicectl_sync_minutes: number;
  strip_watch_apps: boolean;
  app_store_country: string; // two letters, lowercase; default: the Mac's region
  kiwi_builds: boolean; // Kiwi Builds; off until the owner turns it on
}

export interface ApiErrorBody {
  error: { code: string; message: string };
}

// Request and response shapes from the endpoint table.

export interface Health {
  ok: true;
  version: string;
}

/** `PATCH /api/me`. `os_version: null` clears a manual value; `platform: null` clears the choice. */
export interface MePatch {
  name?: string;
  os_version?: string | null;
  platform?: Platform | null;
  /** `false` shows the first-run setup again. */
  onboarded?: boolean;
}

export interface SourcePatch {
  enabled?: boolean;
  browse?: BrowseMode;
}

export interface AppPatch {
  hidden?: boolean;
  /** null: match by bundle id again; { bundle_id: null }: not based on an App Store app. */
  original?: { bundle_id: string | null } | null;
}

export interface PrepareRequest {
  version?: string;
  build?: string;
  revision?: number;
}

export interface PushSubscriptionBody {
  endpoint: string;
  keys: { p256dh: string; auth: string };
}

export interface DevicesResponse {
  devices: Device[];
  core_devices: CoreDevice[];
}

export interface DevicePatch {
  name?: string;
  devicectl_id?: string | null;
}

export interface PushTestResult {
  sent: number;
}
export interface AutomationPreferences {
  enabled: boolean;
  interval_minutes: number;
  tweaks: string[];
  /** Home Screen name, for apps that offer a choice (`names`). */
  name?: string;
}

export interface AutomationJob {
  id: string;
  app: string;
  sequence: number;
  status: "queued" | "running" | "ready" | "unchanged" | "failed" | "blocked" | "cancelled";
  stage: string;
  version: string | null;
  external_id: string | null;
  recipe_key: string | null;
  signed_build_id: string | null;
  error: string | null;
  created_at: string;
  finished_at: string | null;
  preferences?: AutomationPreferences;
}

export interface BuildFeature {
  id: string;
  name: string;
  description: string;
}

export interface BuildLimitation {
  /** An `Icon` name. */
  icon: string;
  title: string;
  detail: string;
}

export interface SetupCheck {
  name: string;
  ready: boolean;
}

/** One app Kiwi can patch and sign on the Mac (`GET /api/automation/:app`). */
export interface AutomationState {
  id: string;
  platform: Platform;
  name: string;
  /** The built app's bundle id, or the package of an Android app. */
  bundle_id: string;
  icon_url: string | null;
  /** The tweak and its version, e.g. "NeoFreeBird 7.3.0". */
  tweak: string;
  summary: string;
  experimental: boolean;
  warning: string | null;
  names: string[];
  /** The version of the original app the tweak supports, when it isn't the newest. */
  app_version: string | null;
  limitations: BuildLimitation[];
  included: BuildFeature[];
  available_tweaks: BuildFeature[];
  added: boolean;
  preferences: AutomationPreferences;
  jobs: AutomationJob[];
  checks: SetupCheck[];
  last_checked_at: string | null;
  last_success_key: string | null;
  last_success_tweaks?: string[] | null;
  last_success_name?: string | null;
  failed_recipe: string | null;
  failed_attempts: number;
  sequence: number;
  published: {
    version: string;
    build: string | null;
    revision: number | null;
    date: string | null;
    tweaks: string[] | null;
    name: string | null;
  } | null;
  source_enabled: boolean | null;
  configuration_changed: boolean;
  next_check_at: string | null;
}

/** Every app Kiwi can build, with its latest jobs (`GET /api/automation`). */
export interface AutomationOverview {
  apps: AutomationState[];
  checks: SetupCheck[];
}
