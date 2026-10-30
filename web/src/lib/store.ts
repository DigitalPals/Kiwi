import type { AppSummary, Original, OriginalMatch } from "../api/types";
import { BoundedCache } from "./cache";
import { parseDate } from "./format";

const DAY = 86_400_000;
const regionNames = new BoundedCache<string, Intl.DisplayNames>(8);

/** "Netherlands" for "nl"; the code itself when the name is unknown. */
export function countryName(code: string, locale = "en"): string {
  try {
    let names = regionNames.get(locale);
    if (!names) {
      names = new Intl.DisplayNames([locale], { type: "region" });
      regionNames.set(locale, names);
    }
    return names.of(code.toUpperCase()) ?? code.toUpperCase();
  } catch {
    return code.toUpperCase();
  }
}

/** The App Store has a newer version than this app. */
export function isBehind(original: Original | null | undefined): original is Original {
  return original?.status === "behind";
}

/** "3 days", "2 weeks", "4 months", "1 year" between two dates; null when unknown or under a day. */
export function timeBetween(from: string | null | undefined, to: string | null | undefined): string | null {
  const a = parseDate(from);
  const b = parseDate(to);
  if (!a || !b) return null;
  const days = Math.floor(Math.abs(b.getTime() - a.getTime()) / DAY);
  const unit = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
  if (days < 1) return null;
  if (days < 14) return unit(days, "day");
  if (days < 60) return unit(Math.round(days / 7), "week");
  if (days < 365) return unit(Math.round(days / 30), "month");
  return unit(Math.round(days / 365), "year");
}

/** The line under "Original App": how the versions compare and how the app was matched. */
export function originalNote(
  app: Pick<AppSummary, "bundle_id" | "original" | "state">,
  match: OriginalMatch,
): string {
  const original = app.original;
  if (!original) return "";
  const released = app.state.target?.date ?? null;
  let comparison: string;
  switch (original.status) {
    case "same":
      comparison = "Same version as the App Store.";
      break;
    case "behind": {
      const gap = timeBetween(released, original.released_at);
      comparison = gap ? `The App Store version is ${gap} newer.` : "The App Store has a newer version.";
      break;
    }
    case "ahead":
      comparison = "This app is newer than the App Store version.";
      break;
    default:
      comparison = "The version numbers can't be compared.";
  }
  const how = match === "linked" ? "Linked by hand." : `Matched by bundle ID ${app.bundle_id}.`;
  return `${comparison} ${how}`;
}

/** A search term for linking an app by hand: its name without "(…)" or "[…]" notes. */
export function searchTerm(name: string): string {
  const plain = name
    .replace(/\s*[([].*?[)\]]\s*/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  return plain || name.trim();
}
