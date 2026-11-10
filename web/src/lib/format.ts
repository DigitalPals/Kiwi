import type { Device, Platform, Release } from "../api/types";
import { BoundedCache } from "./cache";

const DAY = 86_400_000;
const dates = new BoundedCache<string, Intl.DateTimeFormat>(16);
const relativeTimes = new BoundedCache<string | undefined, Intl.RelativeTimeFormat>(8);

function dateFormatter(locale: string | undefined, options: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  const key = JSON.stringify([locale, options]);
  let formatter = dates.get(key);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat(locale, options);
    dates.set(key, formatter);
  }
  return formatter;
}

export interface FormatOptions {
  locale?: string;
  timeZone?: string;
}

/** Decimal units like the App Store ("150.3 MB"). */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1000) return `${Math.round(bytes)} ${bytes === 1 ? "byte" : "bytes"}`;
  const units = [
    { unit: "KB", size: 1e3, digits: 0 },
    { unit: "MB", size: 1e6, digits: 1 },
    { unit: "GB", size: 1e9, digits: 2 },
  ] as const;
  let chosen: (typeof units)[number] = units[0];
  for (const u of units) if (bytes >= u.size) chosen = u;
  // Round first so 999.96 KB becomes "1 MB" rather than "1000 KB".
  let value = Number((bytes / chosen.size).toFixed(chosen.digits));
  if (value >= 1000 && chosen.unit !== "GB") {
    chosen = chosen.unit === "KB" ? units[1] : units[2];
    value = Number((bytes / chosen.size).toFixed(chosen.digits));
  }
  return `${value} ${chosen.unit}`;
}

const DATE_ONLY = /^\d{4}-\d{2}-\d{2}$/;

/** Parses RFC 3339 timestamps and plain dates; returns null for anything invalid. */
export function parseDate(value: string | null | undefined): Date | null {
  if (!value) return null;
  const date = new Date(DATE_ONLY.test(value) ? `${value}T00:00:00Z` : value);
  return Number.isNaN(date.getTime()) ? null : date;
}

/** "Oct 26, 2027". Plain dates are shown as written, independent of the time zone. */
export function formatDate(value: string | null | undefined, options: FormatOptions = {}): string {
  if (!value) return "—";
  const date = parseDate(value);
  if (!date) return value;
  return dateFormatter(options.locale, {
    year: "numeric",
    month: "short",
    day: "numeric",
    timeZone: DATE_ONLY.test(value) ? "UTC" : options.timeZone,
  }).format(date);
}

export function formatDateTime(value: string | null | undefined, options: FormatOptions = {}): string {
  const date = parseDate(value);
  if (!date) return value || "—";
  return dateFormatter(options.locale, {
    dateStyle: "medium",
    timeStyle: "short",
    timeZone: options.timeZone,
  }).format(date);
}

/** "just now", "5 minutes ago", "yesterday", "in 3 days", "2 months ago". */
export function formatRelative(
  value: string | Date | null | undefined,
  now: Date = new Date(),
  locale?: string,
): string {
  const date = value instanceof Date ? value : parseDate(value);
  if (!date) return typeof value === "string" && value ? value : "never";
  const diff = date.getTime() - now.getTime();
  const abs = Math.abs(diff);
  if (abs < 45_000) return "just now";
  let rtf = relativeTimes.get(locale);
  if (!rtf) {
    rtf = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
    relativeTimes.set(locale, rtf);
  }
  const sign = Math.sign(diff);
  const steps: [number, Intl.RelativeTimeFormatUnit][] = [
    [3_600_000, "minute"],
    [DAY, "hour"],
    [30 * DAY, "day"],
    [365 * DAY, "month"],
  ];
  const sizes: Record<string, number> = { minute: 60_000, hour: 3_600_000, day: DAY, month: 30 * DAY, year: 365 * DAY };
  let unit: Intl.RelativeTimeFormatUnit = "year";
  for (const [limit, u] of steps) {
    if (abs < limit) {
      unit = u;
      break;
    }
  }
  const amount = Math.max(1, Math.floor(abs / (sizes[unit] ?? DAY)));
  return rtf.format(sign * amount, unit);
}


export type Tone = "ok" | "warn" | "danger";

export function expiryInfo(
  value: string | null | undefined,
  now: Date = new Date(),
): { label: string; tone: Tone } | null {
  const date = parseDate(value);
  if (!date) return null;
  const ms = date.getTime() - now.getTime();
  if (ms <= 0) {
    const ago = Math.floor(-ms / DAY);
    return { label: ago === 0 ? "Expired today" : `Expired ${plural(ago, "day")} ago`, tone: "danger" };
  }
  const days = Math.floor(ms / DAY);
  const label = days === 0 ? "Expires today" : `Expires in ${plural(days, "day")}`;
  return { label, tone: days < 7 ? "danger" : days < 30 ? "warn" : "ok" };
}

/** Compact time left for the info strip: "6 days", "11 mo", "1 yr", "Today", "Expired". */
export function formatRemaining(value: string | null | undefined, now: Date = new Date()): string | null {
  const date = parseDate(value);
  if (!date) return null;
  const ms = date.getTime() - now.getTime();
  if (ms <= 0) return "Expired";
  const days = Math.floor(ms / DAY);
  if (days === 0) return "Today";
  if (days < 60) return plural(days, "day");
  if (days < 365) return `${Math.floor(days / 30)} mo`;
  return `${Math.floor(days / 365)} yr`;
}

/** "152.4 MB" as ["152.4", "MB"] for a value with its unit underneath. */
export function splitBytes(bytes: number | null | undefined): [string, string] {
  const text = formatBytes(bytes);
  const space = text.lastIndexOf(" ");
  return space > 0 ? [text.slice(0, space), text.slice(space + 1)] : [text, ""];
}

export function plural(count: number, singular: string, pluralForm = `${singular}s`): string {
  return `${count} ${count === 1 ? singular : pluralForm}`;
}

export function formatPercent(progress: number | null | undefined): string | null {
  if (progress === null || progress === undefined || !Number.isFinite(progress)) return null;
  return `${Math.round(Math.min(Math.max(progress, 0), 1) * 100)}%`;
}

/** "https://mac-studio.example.ts.net" -> "Mac Studio"; IP addresses are shown as they are. */
export function serverLabel(publicUrl: string): string {
  let host: string;
  try {
    host = new URL(publicUrl).hostname;
  } catch {
    return publicUrl || "Kiwi";
  }
  if (/^[\d.]+$/.test(host) || host.includes(":") || host.startsWith("[")) return host;
  const words = (host.split(".")[0] ?? host).split(/[-_]+/).filter(Boolean);
  if (words.length === 0) return host;
  return words.map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join(" ");
}

export function formatVersion(release: Pick<Release, "version" | "build" | "revision">): string {
  const version = release.build && release.build !== release.version ? `${release.version} (${release.build})` : release.version;
  return release.revision !== undefined ? `${version} · Kiwi ${release.revision}` : version;
}

export function osVersionSourceLabel(source: Device["os_version_source"]): string {
  switch (source) {
    case "devicectl":
      return "reported by the Mac (devicectl)";
    case "installer":
      return "reported by the installer";
    case "manual":
      return "set manually";
    case "safari":
      return "estimated from Safari";
    case "android":
      return "reported by the browser";
    default:
      return "unknown";
  }
}

const OS_NAMES: Record<Platform, string> = { ios: "iOS", android: "Android" };

/** The operating system of a platform: "iOS" or "Android". */
export const osName = (platform: Platform | undefined): string => OS_NAMES[platform ?? "ios"];

/** What copy calls the device of a platform: "iPhone" or "Android device". */
export const deviceName = (platform: Platform | undefined): string => (platform === "android" ? "Android device" : "iPhone");

/** Short, human summary for a release's OS requirement. */
export function osRequirement(release: Pick<Release, "min_os" | "max_os">, platform?: Platform): string {
  const os = osName(platform);
  if (release.min_os && release.max_os) return `${os} ${release.min_os}–${release.max_os}`;
  if (release.min_os) return `${os} ${release.min_os}+`;
  if (release.max_os) return `Up to ${os} ${release.max_os}`;
  // An Android browser doesn't tell Kiwi the OS version, so Android releases name none.
  return platform === "android" ? os : `Any ${os}`;
}

const PRIVACY_LABELS: Record<string, string> = {
  NSAppleMusicUsageDescription: "Media & Apple Music",
  NSBluetoothAlwaysUsageDescription: "Bluetooth",
  NSBluetoothPeripheralUsageDescription: "Bluetooth",
  NSCalendarsUsageDescription: "Calendars",
  NSCameraUsageDescription: "Camera",
  NSContactsUsageDescription: "Contacts",
  NSFaceIDUsageDescription: "Face ID",
  NSHealthShareUsageDescription: "Health",
  NSLocalNetworkUsageDescription: "Local Network",
  NSLocationAlwaysAndWhenInUseUsageDescription: "Location",
  NSLocationWhenInUseUsageDescription: "Location",
  NSMicrophoneUsageDescription: "Microphone",
  NSMotionUsageDescription: "Motion & Fitness",
  NSPhotoLibraryAddUsageDescription: "Add to Photos",
  NSPhotoLibraryUsageDescription: "Photos",
  NSRemindersUsageDescription: "Reminders",
  NSSpeechRecognitionUsageDescription: "Speech Recognition",
  NSUserTrackingUsageDescription: "Tracking",
};

/** Friendly name for an Info.plist privacy key ("NSCameraUsageDescription" -> "Camera"). */
export function privacyLabel(key: string): string {
  const known = PRIVACY_LABELS[key];
  if (known) return known;
  const core = key.replace(/^NS/, "").replace(/UsageDescription$/, "");
  return core.replace(/([a-z])([A-Z])/g, "$1 $2").trim() || key;
}
