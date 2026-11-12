import type {
  AppDetail,
  CoreDevice,
  Device,
  Identity,
  Platform,
  Profile,
  Settings,
  SigningBackend,
  Source,
  StoreApp,
} from "../src/api/types.ts";

export const TEAM_ID = "A1B2C3D4E5";
export const DAY = 86_400_000;
export const PROFILE_EXPIRES = Date.parse("2027-10-26T09:41:00Z");
export const CERT_EXPIRES = Date.parse("2027-10-25T18:12:09Z");

/** A release as written in a source; compatibility is computed per device. */
export interface MockRelease {
  version: string;
  build: string | null;
  /** Kiwi Builds' own revision of a version. */
  revision?: number;
  date: string | null;
  notes: string | null;
  size: number | null;
  min_os: string | null;
  max_os: string | null;
}

export interface MockApp {
  source_id: string;
  bundle_id: string;
  slug: string;
  name: string;
  developer: string | null;
  subtitle: string | null;
  tint_color: string | null;
  category: string | null;
  description: string | null;
  screenshots: number;
  releases: MockRelease[];
  permissions: AppDetail["permissions"];
  dropped_entitlements: string[];
  /** Shown in or hidden from Browse by hand; undefined follows the source's mode. */
  shown?: boolean;
  /** Added to a "picked" source after it was switched; cleared once chosen. */
  is_new?: boolean;
  /** Original app: undefined matches by bundle id, null is none, a string links that App Store app. */
  original?: string | null;
  /** "android": an APK built by Kiwi Builds, whose `bundle_id` is its package. */
  platform?: Platform;
}

/** Counts are computed from the apps. */
export type MockSource = Omit<Source, "shown_count" | "new_count" | "kind" | "ipa_url">;

/** An App Store app; `slug` names its placeholder icon. */
export interface MockStoreApp extends Omit<StoreApp, "short_name" | "icon_url"> {
  slug: string;
  tint: string;
}

export interface MockInstall {
  app_key: string;
  version: string;
  build: string | null;
  revision?: number;
  origin: "kiwi" | "device";
  verified: boolean;
  installed_at: number;
  expires_at: number | null;
  kiwi_scheme: boolean;
}

export interface MockBuild {
  id: string;
  app_key: string;
  version: string;
  build: string | null;
  revision?: number;
  started: number;
  duration: number;
  /** Fails during signing with this error. */
  error: string | null;
  size: number;
  dropped: string[];
  token: string;
}

export interface MockAttempt {
  token: string;
  build_id: string;
  manifest_at: number | null;
  install_ms: number;
  completed_at: number | null;
}

export interface MockDb {
  device: Device;
  /** What Tailscale or the browser told about the device's platform (null: a desktop browser). */
  detected_platform: Platform | null;
  /** The owner's choice of platform (`PATCH /api/me`). */
  chosen_platform: Platform | null;
  /** The first-run setup was completed on this device. */
  onboarded: boolean;
  /** Android packages installed with another signature (`PUT /api/me/packages`). */
  foreign_packages: Set<string>;
  installer_os: string;
  settings: Settings;
  sources: MockSource[];
  /** App Store apps by storefront. */
  store: Record<string, MockStoreApp[]>;
  apps: MockApp[];
  installs: MockInstall[];
  builds: MockBuild[];
  attempts: MockAttempt[];
  subscriptions: Set<string>;
  core_devices: CoreDevice[];
  signing_backend: SigningBackend;
  identities: Identity[];
  profiles: Profile[];
}

const iso = (ms: number) => new Date(ms).toISOString();
const MB = 1_000_000;

const YOUTUBE_NOTES = `• Updated to YouTube 21.44.0
• Fixed Picture-in-Picture on iOS 27
• New option: hide Shorts shelf in subscriptions
• Faster startup when many tweaks are enabled`;

const LONG_DESCRIPTION = `YouTubePlus (YTLite) adds the features YouTube is missing.

Features:
• Download videos and audio in any quality
• Background playback and Picture-in-Picture
• Block ads, sponsor segments and Shorts
• Customise the player, tabs and gestures
• Restore the dislike count

Settings are under You › YouTubePlus. Some features need a restart of the app.

This build is unofficial and not affiliated with Google. Please report problems to the repository maintainer rather than to YouTube.`;

function app(partial: Partial<MockApp> & Pick<MockApp, "source_id" | "bundle_id" | "slug" | "name" | "releases">): MockApp {
  return {
    developer: null,
    subtitle: null,
    tint_color: null,
    category: null,
    description: null,
    screenshots: 0,
    permissions: { entitlements: [], privacy: [] },
    dropped_entitlements: [],
    ...partial,
  };
}

function rel(version: string, build: string | null, daysAgo: number, now: number, extra: Partial<MockRelease> = {}): MockRelease {
  return {
    version,
    build,
    date: iso(now - daysAgo * DAY).slice(0, 10),
    notes: null,
    size: null,
    min_os: "15.0",
    max_os: null,
    ...extra,
  };
}

/** `platform`: the device the mock answers as; an Android one starts with nothing installed. */
export function createDb(now: number, platform: Platform = "ios", signing: SigningBackend = "keychain"): MockDb {
  const sources: MockSource[] = [
    {
      id: "ytlite",
      url: "https://raw.githubusercontent.com/mrdrvt99/Altstore-Repository/main/ytlite.json",
      name: "YTLite Repository",
      subtitle: "Tweaked apps by mrdrvt99",
      icon_url: "/__mock/icon/ytlite.svg",
      enabled: true,
      browse: "all",
      app_count: 4,
      added_at: iso(now - 40 * DAY),
      last_attempt_at: iso(now - 12 * 60_000),
      last_success_at: iso(now - 12 * 60_000),
      error: null,
      diagnostics: [],
    },
    {
      id: "altstore",
      url: "https://apps.altstore.io",
      name: "AltStore",
      subtitle: "Official AltStore apps",
      icon_url: "/__mock/icon/altstore.svg",
      enabled: true,
      browse: "all",
      app_count: 1,
      added_at: iso(now - 38 * DAY),
      last_attempt_at: iso(now - 12 * 60_000),
      last_success_at: iso(now - 12 * 60_000),
      error: null,
      diagnostics: [],
    },
    {
      id: "emulators",
      url: "https://raw.githubusercontent.com/example/emulators/main/apps.json",
      name: "Emulators & Tools",
      subtitle: null,
      icon_url: null,
      enabled: true,
      browse: "all",
      app_count: 5,
      added_at: iso(now - 20 * DAY),
      last_attempt_at: iso(now - 13 * 60_000),
      last_success_at: iso(now - 13 * 60_000),
      error: null,
      diagnostics: [
        'App "RetroArch": release 1.19.1 has no downloadURL; skipped',
        'Unknown top-level field "featuredApps" ignored',
        'App "iSH": "size" is a string; parsed as 18400000',
      ],
    },
    {
      id: "youmod",
      url: "https://raw.githubusercontent.com/example/youmod/main/apps.json",
      name: "YouMod Repo",
      subtitle: null,
      icon_url: null,
      enabled: true,
      browse: "all",
      app_count: 1,
      added_at: iso(now - 5 * DAY),
      last_attempt_at: iso(now - 15 * 60_000),
      last_success_at: iso(now - 15 * 60_000),
      error: null,
      diagnostics: [],
    },
    {
      id: "broken",
      url: "https://repo.example.org/altstore/source.json",
      name: "Nightly Builds",
      subtitle: null,
      icon_url: null,
      enabled: true,
      browse: "all",
      app_count: 0,
      added_at: iso(now - 9 * DAY),
      last_attempt_at: iso(now - 14 * 60_000),
      last_success_at: iso(now - 3 * DAY),
      error: "HTTP 404 Not Found from repo.example.org",
      diagnostics: [],
    },
    {
      id: "archive",
      url: "https://raw.githubusercontent.com/example/archive/main/source.json",
      name: "Old Archive",
      subtitle: "Unmaintained",
      icon_url: null,
      enabled: false,
      browse: "all",
      app_count: 12,
      added_at: iso(now - 120 * DAY),
      last_attempt_at: iso(now - 60 * DAY),
      last_success_at: iso(now - 60 * DAY),
      error: null,
      diagnostics: [],
    },
    {
      id: "driftywinds",
      url: "https://raw.githubusercontent.com/driftywinds/driftywinds.github.io/master/AltStore/apps.json",
      name: "driftywinds' AltStore Repo",
      subtitle: null,
      icon_url: null,
      enabled: true,
      browse: "picked",
      app_count: 10,
      added_at: iso(now - 3 * DAY),
      last_attempt_at: iso(now - 11 * 60_000),
      last_success_at: iso(now - 11 * 60_000),
      error: null,
      diagnostics: [],
    },
  ];

  const apps: MockApp[] = [
    app({
      source_id: "ytlite",
      bundle_id: "com.google.ios.youtube",
      slug: "youtube",
      name: "YouTubePlus",
      developer: "dayanch96",
      subtitle: "YouTube with downloads and no ads",
      tint_color: "#FF0033",
      category: "entertainment",
      description: LONG_DESCRIPTION,
      screenshots: 4,
      releases: [
        rel("21.44.0", "21.44.0", 1, now, { notes: YOUTUBE_NOTES, size: 152.4 * MB }),
        rel("21.43.5", "21.43.5", 9, now, { notes: "• Updated to YouTube 21.43.5", size: 150.3 * MB }),
        rel("21.40.2", "21.40.2", 30, now, { notes: "• Initial YTLite 5.2 build", size: 148.9 * MB }),
      ],
      permissions: {
        entitlements: [
          "application-identifier",
          "com.apple.security.application-groups",
          "keychain-access-groups",
          "com.apple.developer.associated-domains",
        ],
        privacy: [
          { key: "NSCameraUsageDescription", description: "Record and upload videos." },
          { key: "NSMicrophoneUsageDescription", description: "Record audio for your videos and voice search." },
          { key: "NSPhotoLibraryAddUsageDescription", description: "Save downloaded videos to your library." },
          { key: "NSLocalNetworkUsageDescription", description: "Cast to TVs on your network." },
        ],
      },
      dropped_entitlements: ["com.apple.developer.associated-domains", "com.apple.security.application-groups"],
    }),
    app({
      source_id: "ytlite",
      bundle_id: "com.atebits.Tweetie2",
      slug: "bhtwitter",
      name: "BHTwitter",
      developer: "BandarHL",
      subtitle: "Twitter with downloads and more",
      tint_color: "#1DA1F2",
      category: "social",
      description: "Download videos, hide ads and promoted tweets, and customise the timeline.",
      screenshots: 3,
      releases: [rel("11.21", "11.21.1", 4, now, { notes: "Updated to X 11.21", size: 118.2 * MB })],
      permissions: {
        entitlements: ["application-identifier", "aps-environment"],
        privacy: [{ key: "NSPhotoLibraryUsageDescription", description: "Attach photos to posts." }],
      },
      dropped_entitlements: ["aps-environment"],
    }),
    app({
      source_id: "ytlite",
      bundle_id: "com.spotify.client",
      slug: "spotify",
      name: "EeveeSpotify",
      developer: "whoeevee",
      subtitle: "Spotify Premium features",
      tint_color: "#1DB954",
      category: "entertainment",
      description: "Lyrics from multiple providers, no ads and unlimited skips.",
      screenshots: 2,
      releases: [rel("9.0.92", "9.0.92", 2, now, { notes: "Spotify 9.0.92 with Eevee 6.2", size: 98.1 * MB })],
    }),
    app({
      source_id: "ytlite",
      bundle_id: "com.christianselig.Apollo",
      slug: "apollo",
      name: "Apollo",
      developer: "Christian Selig",
      subtitle: "Reddit client with ImprovedCustomApi",
      tint_color: "#2B7BFF",
      category: "social",
      description: "The beloved Reddit client, patched to work with your own API key.",
      screenshots: 3,
      releases: [rel("1.15.11", "212", 80, now, { notes: "ImprovedCustomApi 1.3.2", size: 74.6 * MB })],
    }),
    app({
      source_id: "altstore",
      bundle_id: "com.rileytestut.Delta",
      slug: "delta",
      name: "Delta",
      developer: "Riley Testut",
      subtitle: "Classic games in your pocket",
      tint_color: "#8A28F7",
      category: "games",
      description: "Delta is an all-in-one emulator for iOS.\n\nSupports NES, SNES, N64, GBC, GBA, DS and Genesis.",
      screenshots: 4,
      releases: [
        rel("1.7.1", "126", 6, now, { notes: "• Fixed controller skins on iPhone 17\n• Faster DS loading", size: 61.2 * MB }),
        rel("1.7", "125", 40, now, { notes: "• Online multiplayer for DS", size: 60.9 * MB }),
      ],
    }),
    app({
      source_id: "emulators",
      bundle_id: "org.provenance-emu.provenance",
      slug: "provenance",
      name: "Provenance",
      developer: "Provenance Emu",
      subtitle: "Multi-system emulator",
      tint_color: "#E85D2C",
      category: "games",
      releases: [rel("3.1.0", "3100", 5, now, { notes: "PlayStation and Saturn cores updated", size: 210.5 * MB })],
    }),
    app({
      source_id: "emulators",
      bundle_id: "com.utmapp.UTM-SE",
      slug: "utm",
      name: "UTM SE",
      developer: "UTM",
      subtitle: "Virtual machines for iOS",
      tint_color: "#2D6CDF",
      category: "developer",
      description: "Run Windows, Linux and more. UTM SE uses a threaded interpreter and works without JIT.",
      screenshots: 2,
      releases: [rel("4.6.5", "112", 15, now, { notes: "Bug fixes", size: 312.8 * MB })],
    }),
    app({
      source_id: "emulators",
      bundle_id: "org.xbmc.kodi-ios",
      slug: "kodi",
      name: "Kodi",
      developer: "XBMC Foundation",
      subtitle: "Media center",
      tint_color: "#17B2E7",
      category: "entertainment",
      releases: [rel("21.2", "21.2.0", 50, now, { size: 88.0 * MB })],
    }),
    app({
      source_id: "emulators",
      bundle_id: "app.ish.iSH",
      slug: "ish",
      name: "iSH",
      developer: "Theodore Dubois",
      subtitle: "Linux shell",
      tint_color: "#3B3B3B",
      category: "developer",
      releases: [rel("2.0", "600", 3, now, { min_os: "28.0", size: 18.4 * MB, notes: "Requires the iOS 28 SDK" })],
    }),
    app({
      source_id: "emulators",
      bundle_id: "org.ppsspp.ppsspp",
      slug: "ppsspp",
      name: "PPSSPP",
      developer: "Henrik Rydgård",
      subtitle: "PSP emulator",
      tint_color: "#33A0FF",
      category: "games",
      releases: [rel("1.19.3", "1.19.3", 8, now, { size: 45.0 * MB, notes: "Vulkan fixes" })],
    }),
    app({
      source_id: "emulators",
      bundle_id: "com.libretro.dist.ios.RetroArch",
      slug: "retroarch",
      name: "RetroArch",
      developer: "Libretro",
      subtitle: "Frontend for emulators and game engines",
      tint_color: "#4A4A4A",
      category: "games",
      releases: [rel("1.19.0", "1.19.0", 25, now, { size: 140.0 * MB })],
    }),
    // Another YouTube tweak with the same bundle id: installing it replaces YouTubePlus.
    app({
      source_id: "youmod",
      bundle_id: "com.google.ios.youtube",
      slug: "youmod",
      name: "YouMod",
      developer: "example",
      subtitle: "Another YouTube tweak",
      tint_color: "#E62117",
      category: "entertainment",
      releases: [rel("21.44.0", "21.44.0", 2, now, { notes: "YouTube 21.44.0", size: 149.7 * MB })],
    }),
    // driftywinds: a grab bag of tweaked apps, set to "Apps I Pick" with four picked.
    app({
      source_id: "driftywinds",
      bundle_id: "com.google.ios.youtube",
      slug: "dw-youmod",
      name: "YouMod + Tweaks (driftywinds)",
      developer: "Tonwalter888",
      subtitle: "A flexible enhancer for YouTube on iOS | If you want to donate: - https://ko-fi.com/driftywinds",
      tint_color: "#E22A41",
      releases: [rel("21.43.5", null, 0, now, { notes: "• Updated YouTube to 21.43.5\n• Tweak is using 2.0.0", size: 130.1 * MB })],
      shown: true,
    }),
    app({
      source_id: "driftywinds",
      bundle_id: "com.spotify.client",
      slug: "dw-spotify",
      name: "Spotify (EeveeSpotifyReincarnated)",
      developer: "jaydenjcpy & faroukbmiled",
      subtitle: "Spotify with free premium, and extra UI features | If you want to donate: - https://ko-fi.com/driftywinds",
      tint_color: "#1DB954",
      releases: [rel("9.1.50", null, 139, now, { notes: "This tweak makes Spotify think you have a Premium subscription.", size: 91.8 * MB })],
      shown: true,
    }),
    app({
      source_id: "driftywinds",
      bundle_id: "com.google.ios.youtubemusic",
      slug: "dw-ytmusic",
      name: "YT Music Ultimate",
      developer: "Ginsu / dayanch96",
      subtitle: "No ads. Cool tweaks. Requires iOS 15 and later.",
      tint_color: "#C3352E",
      releases: [rel("9.22.2", null, 149, now, { notes: "- updated youtube music to v9.22.2", size: 4.6 * MB })],
      shown: true,
    }),
    app({
      source_id: "driftywinds",
      bundle_id: "kh.crysalis.feather",
      slug: "dw-feather",
      name: "Feather",
      developer: "khcrysalis",
      subtitle: "Feather is a free on-device iOS application manager/installer",
      tint_color: "#5861BD",
      releases: [rel("2.4.0", null, 397, now, { size: 91.8 * MB })],
      shown: true,
    }),
    app({
      source_id: "driftywinds",
      bundle_id: "com.mojang.minecraftpe",
      slug: "dw-minecraft",
      name: "Minecraft",
      developer: "Kamo Hacks",
      subtitle: "Cracked Minecraft with working online",
      tint_color: "#996747",
      releases: [rel("1.26.0", null, 247, now, { size: 91.8 * MB })],
    }),
    app({
      source_id: "driftywinds",
      bundle_id: "com.burbn.instagram",
      slug: "dw-instagram",
      name: "Instagram (RyukGram)",
      developer: "Ryuk",
      subtitle: "Additional features and fixes by Ryuk based on SCInsta",
      tint_color: "#C13584",
      releases: [rel("453.0.0", null, 3, now, { notes: "Updated to v453.0.0, RyukGram v1.5.0", size: 91.8 * MB })],
    }),
    app({
      source_id: "driftywinds",
      bundle_id: "com.atebits.Tweetie2",
      slug: "dw-twitter",
      name: "Twitter BHX",
      developer: "baterblx",
      subtitle: "Awesome tweak for X",
      tint_color: "#1DA1F2",
      releases: [rel("11.54", null, 297, now, { size: 161.0 * MB })],
    }),
    app({
      source_id: "driftywinds",
      bundle_id: "com.unofficial.balatro",
      slug: "dw-balatro",
      name: "Balatro",
      developer: "driftywinds",
      subtitle: "The best rogue-like card game!",
      tint_color: "#6B06C9",
      releases: [rel("1.0.1o", null, 610, now, { size: 91.8 * MB })],
    }),
    app({
      source_id: "driftywinds",
      bundle_id: "com.elgato.streamdeck-ios",
      slug: "dw-streamdeck",
      name: "Elgato Stream Deck Mobile",
      developer: "zxcvbn",
      subtitle: "cracked by zxcvbn",
      tint_color: "#778FF7",
      releases: [rel("2.2.0", null, 900, now, { size: 91.8 * MB })],
    }),
    app({
      source_id: "driftywinds",
      bundle_id: "com.kdt.livecontainer",
      slug: "dw-livecontainer",
      name: "LiveContainer",
      developer: "khanhduytran0",
      subtitle: "Run iOS apps without actually installing them!",
      tint_color: "#8E8E93",
      releases: [rel("2.1.2", null, 1, now, { size: 91.8 * MB })],
      is_new: true,
    }),
    // From a source that was removed; only visible in the library.
    app({
      source_id: "cercube-repo",
      bundle_id: "com.google.ios.youtube.cercube",
      slug: "cercube",
      name: "Cercube",
      developer: "Majd Alfhaily",
      tint_color: "#C4302B",
      releases: [rel("5.3.11", "5.3.11", 200, now, { size: 120.0 * MB })],
    }),
  ];

  const key = (source: string, bundle: string) => `${source}/${bundle}`;
  const installs: MockInstall[] = [
    {
      app_key: key("ytlite", "com.google.ios.youtube"),
      version: "21.43.5",
      build: "21.43.5",
      origin: "kiwi",
      verified: true,
      installed_at: now - 8 * DAY,
      expires_at: PROFILE_EXPIRES,
      kiwi_scheme: true,
    },
    {
      app_key: key("ytlite", "com.christianselig.Apollo"),
      version: "1.15.11",
      build: "212",
      origin: "kiwi",
      verified: true,
      installed_at: now - 359 * DAY,
      expires_at: now + 6 * DAY,
      kiwi_scheme: true,
    },
    {
      app_key: key("emulators", "com.utmapp.UTM-SE"),
      version: "4.6.5",
      build: "112",
      origin: "kiwi",
      verified: false,
      installed_at: now - 14 * DAY,
      expires_at: PROFILE_EXPIRES,
      kiwi_scheme: true,
    },
    {
      app_key: key("emulators", "org.xbmc.kodi-ios"),
      version: "21.2",
      build: "21.2.0",
      origin: "device",
      verified: true,
      installed_at: now - 30 * DAY,
      expires_at: null,
      kiwi_scheme: false,
    },
    {
      app_key: key("cercube-repo", "com.google.ios.youtube.cercube"),
      version: "5.3.11",
      build: "5.3.11",
      origin: "kiwi",
      verified: false,
      installed_at: now - 100 * DAY,
      expires_at: PROFILE_EXPIRES,
      kiwi_scheme: true,
    },
  ];

  const builds: MockBuild[] = [
    {
      id: "b-spotify",
      app_key: key("ytlite", "com.spotify.client"),
      version: "9.0.92",
      build: "9.0.92",
      started: now - 4000,
      duration: 45_000,
      error: null,
      size: 97.4 * MB,
      dropped: [],
      token: "t-spotify",
    },
    {
      id: "b-delta",
      app_key: key("altstore", "com.rileytestut.Delta"),
      version: "1.7.1",
      build: "126",
      started: now - 3 * 60_000,
      duration: 20_000,
      error: null,
      size: 60.7 * MB,
      dropped: [],
      token: "t-delta",
    },
    {
      id: "b-provenance",
      app_key: key("emulators", "org.provenance-emu.provenance"),
      version: "3.1.0",
      build: "3100",
      started: now - 10 * 60_000,
      duration: 20_000,
      error: null,
      size: 209.9 * MB,
      dropped: [],
      token: "t-provenance",
    },
    {
      id: "b-apollo",
      app_key: key("ytlite", "com.christianselig.Apollo"),
      version: "1.15.11",
      build: "212",
      started: now - 20 * 60_000,
      duration: 20_000,
      error: null,
      size: 74.1 * MB,
      dropped: [],
      token: "t-apollo",
    },
    {
      id: "b-ppsspp",
      app_key: key("emulators", "org.ppsspp.ppsspp"),
      version: "1.19.3",
      build: "1.19.3",
      started: now - 30 * 60_000,
      duration: 20_000,
      error: "codesign failed for Frameworks/libMoltenVK.dylib: errSecInternalComponent. Is the Mac logged in?",
      size: 44.6 * MB,
      dropped: [],
      token: "t-ppsspp",
    },
  ];

  const attempts: MockAttempt[] = [
    { token: "t-provenance", build_id: "b-provenance", manifest_at: now - 2000, install_ms: 25_000, completed_at: null },
  ];

  const android = platform === "android";
  const device: Device = android
    ? {
        id: "ts:nANDROID1CNTRL",
        name: "Pixel 9a",
        kind: "tailscale",
        platform: "android",
        platform_source: "detected",
        detected_platform: "android",
        client: "web",
        tailscale_name: "pixel-9a",
        // Chrome doesn't report the Android version.
        os_version: null,
        os_version_source: null,
        model: "Android",
        devicectl: null,
        last_seen: iso(now),
      }
    : {
        id: "ts:nPHONE1CNTRL",
        name: "Alex’s iPhone",
        kind: "tailscale",
        platform: "ios",
        platform_source: "detected",
        detected_platform: "ios",
        client: "web",
        tailscale_name: "iphone194",
        os_version: "27.0.1",
        os_version_source: "installer",
        model: "iPhone19,4",
        devicectl: null,
        last_seen: iso(now),
      };

  const core_devices: CoreDevice[] = [
    {
      id: "8C2F1E54-7A0B-4E5D-9F11-2D6B3C4A5E70",
      name: "Alex's iPhone",
      udid: "00008150-001A2B3C4D5E801E",
      model: "iPhone19,4",
      platform: "iOS",
      os_version: "27.0.1",
      physical: true,
      pairing_state: "unpaired",
      tunnel_state: "disconnected",
      transport: null,
      linked_device_id: null,
    },
    {
      id: "4F1C2A7E-0B3D-4C59-8E21-7A6D5B4C3F10",
      name: "iPhone 17 Pro",
      udid: null,
      model: "iPhone18,1",
      platform: "iOS",
      os_version: "27.0",
      physical: false,
      pairing_state: "paired",
      tunnel_state: "unavailable",
      transport: null,
      linked_device_id: null,
    },
    {
      id: "D9A3E6B1-5C2F-4E7A-8B10-3F4E5D6C7B80",
      name: "iPad Air 13-inch (M3)",
      udid: null,
      model: "iPad15,5",
      platform: "iOS",
      os_version: "27.0",
      physical: false,
      pairing_state: "paired",
      tunnel_state: "unavailable",
      transport: null,
      linked_device_id: null,
    },
  ];

  const base = ["application-identifier", "com.apple.developer.team-identifier", "get-task-allow", "keychain-access-groups"];
  const profiles: Profile[] = [
    {
      uuid: "9f1c2b3a-4d5e-4f60-8a7b-0c1d2e3f4a5b",
      name: "iOS Team Provisioning Profile: *",
      team_id: TEAM_ID,
      app_id_pattern: `${TEAM_ID}.*`,
      wildcard: true,
      development: true,
      created_at: "2026-10-26T09:41:00Z",
      expires_at: iso(PROFILE_EXPIRES),
      devices: 2,
      entitlements: base,
      usable: true,
      reason: null,
      uploaded: false,
    },
    {
      uuid: "5e0c7d1a-93b4-4f2e-a8c6-1d2e3f4a5b6c",
      name: "iOS Team Provisioning Profile: com.example.Notes",
      team_id: TEAM_ID,
      app_id_pattern: `${TEAM_ID}.com.example.Notes`,
      wildcard: false,
      development: true,
      created_at: "2026-09-02T11:20:00Z",
      expires_at: "2027-09-02T11:20:00Z",
      devices: 2,
      entitlements: [...base, "aps-environment", "com.apple.security.application-groups"],
      usable: true,
      reason: null,
      uploaded: false,
    },
    {
      uuid: "c41f8e2b-6a7d-4b90-9e3c-5f6a7b8c9d0e",
      name: "iOS Team Provisioning Profile: com.example.Scratch",
      team_id: TEAM_ID,
      app_id_pattern: `${TEAM_ID}.com.example.Scratch`,
      wildcard: false,
      development: true,
      created_at: "2025-05-01T08:00:00Z",
      expires_at: "2026-05-01T08:00:00Z",
      devices: 1,
      entitlements: base,
      usable: false,
      reason: "Expired on May 1, 2026",
      uploaded: false,
    },
    {
      uuid: "0b9a8c7d-6e5f-4a3b-2c1d-0e9f8a7b6c5d",
      name: "iOS Team Provisioning Profile: *",
      team_id: TEAM_ID,
      app_id_pattern: `${TEAM_ID}.*`,
      wildcard: true,
      development: true,
      created_at: "2025-11-10T10:00:00Z",
      expires_at: "2026-11-10T10:00:00Z",
      devices: 1,
      entitlements: base,
      usable: false,
      reason: "Does not include the signing certificate 45ADD89D…",
      uploaded: false,
    },
  ];

  // Apple's iTunes lookup answers (versions and names as of late October 2026).
  const daysAgo = (n: number) => iso(now - n * DAY);
  const nl: MockStoreApp[] = [
    { slug: "store-youtube", tint: "#FF0033", bundle_id: "com.google.ios.youtube", name: "YouTube", developer: "Google", version: "21.44.0", released_at: daysAgo(1), store_url: "https://apps.apple.com/nl/app/youtube/id544007664" },
    { slug: "store-spotify", tint: "#1DB954", bundle_id: "com.spotify.client", name: "Spotify: Muziek en podcasts", developer: "Spotify", version: "9.1.94", released_at: daysAgo(6), store_url: "https://apps.apple.com/nl/app/spotify-muziek-en-podcasts/id324684580" },
    { slug: "store-x", tint: "#111111", bundle_id: "com.atebits.Tweetie2", name: "X", developer: "X Corp.", version: "12.38.1", released_at: daysAgo(2), store_url: "https://apps.apple.com/nl/app/x/id333903271" },
    { slug: "store-ytmusic", tint: "#FF0033", bundle_id: "com.google.ios.youtubemusic", name: "YouTube Music", developer: "Google", version: "9.43", released_at: daysAgo(4), store_url: "https://apps.apple.com/nl/app/youtube-music/id1017492454" },
    { slug: "store-ytkids", tint: "#FF0033", bundle_id: "com.google.ios.youtubekids", name: "YouTube Kids", developer: "Google", version: "11.43.805", released_at: daysAgo(3), store_url: "https://apps.apple.com/nl/app/youtube-kids/id936971630" },
    { slug: "store-instagram", tint: "#C13584", bundle_id: "com.burbn.instagram", name: "Instagram", developer: "Instagram, Inc.", version: "453.0", released_at: daysAgo(3), store_url: "https://apps.apple.com/nl/app/instagram/id389801252" },
    { slug: "store-minecraft", tint: "#5B8C32", bundle_id: "com.mojang.minecraftpe", name: "Minecraft: Speel met vrienden", developer: "Mojang", version: "26.60", released_at: daysAgo(2), store_url: "https://apps.apple.com/nl/app/minecraft/id479516143" },
    { slug: "store-streamdeck", tint: "#3D5AFE", bundle_id: "com.elgato.streamdeck-ios", name: "Elgato Stream Deck Mobile", developer: "Corsair", version: "2.4.2", released_at: daysAgo(98), store_url: "https://apps.apple.com/nl/app/elgato-stream-deck-mobile/id1440014184" },
    { slug: "store-delta", tint: "#8A28F7", bundle_id: "com.rileytestut.Delta", name: "Delta - Game Emulator", developer: "Riley Testut", version: "1.7.1", released_at: daysAgo(6), store_url: "https://apps.apple.com/nl/app/delta-game-emulator/id1048524688" },
    { slug: "store-utm", tint: "#2D6CDF", bundle_id: "com.utmapp.UTM-SE", name: "UTM SE: Retro PC Emulator", developer: "Turing Software", version: "4.6.5", released_at: daysAgo(15), store_url: "https://apps.apple.com/nl/app/utm-se-retro-pc-emulator/id1564628856" },
    { slug: "store-ish", tint: "#3B3B3B", bundle_id: "app.ish.iSH", name: "iSH Shell", developer: "Theodore Dubois", version: "1.3.2", released_at: daysAgo(400), store_url: "https://apps.apple.com/nl/app/ish-shell/id1436902243" },
    { slug: "store-ppsspp", tint: "#33A0FF", bundle_id: "org.ppsspp.ppsspp", name: "PPSSPP - PSP emulator", developer: "Henrik Rydgard", version: "1.19.3", released_at: daysAgo(8), store_url: "https://apps.apple.com/nl/app/ppsspp-psp-emulator/id6496972903" },
    { slug: "store-retroarch", tint: "#4A4A4A", bundle_id: "com.libretro.dist.ios.RetroArch", name: "RetroArch", developer: "Libretro", version: "1.19.1", released_at: daysAgo(12), store_url: "https://apps.apple.com/nl/app/retroarch/id6499539433" },
  ];
  const english: Record<string, string> = {
    "com.spotify.client": "Spotify: Music and Podcasts",
    "com.mojang.minecraftpe": "Minecraft: Play with Friends!",
  };
  const us = nl.map((a) => ({
    ...a,
    name: english[a.bundle_id] ?? a.name,
    store_url: a.store_url.replace("/nl/", "/us/"),
  }));

  return {
    device,
    detected_platform: platform,
    chosen_platform: null,
    onboarded: true,
    foreign_packages: new Set(),
    installer_os: "27.0.1",
    store: { nl, us },
    settings: {
      refresh_interval_minutes: 60,
      auto_prepare_updates: true,
      notify_updates: true,
      notify_expiry: true,
      devicectl_sync: true,
      devicectl_sync_minutes: 10,
      strip_watch_apps: true,
      app_store_country: "nl",
      kiwi_builds: true,
      follow_tweak_updates: true,
    },
    sources,
    apps,
    // The fixture installs are the iPhone's.
    installs: android ? [] : installs,
    builds,
    attempts: android ? [] : attempts,
    subscriptions: new Set(),
    core_devices,
    signing_backend: signing,
    identities: [
      {
        sha1: "0123456789ABCDEF0123456789ABCDEF01234567",
        name: "Apple Development: Alex Morgan (Z9Y8X7W6V5)",
        team_id: TEAM_ID,
        expires_at: iso(CERT_EXPIRES),
      },
    ],
    // A Kiwi on Linux has only the profiles that were uploaded to it.
    profiles: signing === "files" ? profiles.map((p) => ({ ...p, uploaded: true })) : profiles,
  };
}
