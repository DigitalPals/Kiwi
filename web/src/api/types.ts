// Mirrors docs/api.md exactly. Keep in sync with crates/kiwi-server.

export type Timestamp = string;

export interface Device {
  id: string; // "ts:<StableID>" | "local:<random>"
  name: string;
  kind: "tailscale" | "browser";
  tailscale_name: string | null;
  os_version: string | null;
  os_version_source: "devicectl" | "installer" | "manual" | "safari" | null;
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

export interface Me {
  device: Device;
  server: {
    version: string;
    public_url: string;
    team_id: string | null;
    devicectl_available: boolean;
  };
  push: { vapid_public_key: string; subscribed: boolean };
  counts: { updates: number; installed: number; sources: number; source_errors: number; preparing: number };
  warnings: Warning[];
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
}

export interface Signing {
  identities: Identity[];
  profiles: Profile[];
  active: { identity_sha1: string; profile_uuid: string; expires_at: Timestamp } | null;
  error: string | null;
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

export interface MePatch {
  name?: string;
  os_version?: string | null;
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
  name: string;
  bundle_id: string;
  icon_url: string | null;
  /** The tweak and its version, e.g. "NeoFreeBird 7.3.0". */
  tweak: string;
  summary: string;
  experimental: boolean;
  warning: string | null;
  names: string[];
  /** The App Store version the tweak supports, when it isn't the newest. */
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
