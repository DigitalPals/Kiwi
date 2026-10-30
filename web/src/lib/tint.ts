// Colour comes from the apps themselves: their source's `tintColor`, or a colour picked from
// the name when a source has none. Evergreen stays the colour of actions.
import type { Installed } from "../api/types";
import { parseDate } from "./format";

const DAY = 86_400_000;
const HEX = /^#?[0-9a-f]{6}$/i;

/** Mid-tone colours that carry white text, for apps (and sources) without a tint. */
const PALETTE = ["#2b7bff", "#8a28f7", "#e85d2c", "#1e9e6a", "#d6336c", "#0f8fb0", "#c27c0e", "#5861bd"];

type Rgb = [number, number, number];

/** A source's `tintColor` as "#rrggbb", or null when it isn't a 6-digit hex colour. */
export function parseTint(tint: string | null | undefined): string | null {
  if (!tint || !HEX.test(tint)) return null;
  return (tint.startsWith("#") ? tint : `#${tint}`).toLowerCase();
}

/** The app's tint, else the same palette colour for the same name every time. */
export function appColor(name: string, tint: string | null | undefined): string {
  const parsed = parseTint(tint);
  if (parsed) return parsed;
  let hash = 0;
  for (const char of name) hash = (hash * 31 + (char.codePointAt(0) ?? 0)) >>> 0;
  return PALETTE[hash % PALETTE.length] as string;
}

function rgb(hex: string): Rgb {
  const n = Number.parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function hex([r, g, b]: Rgb): string {
  return `#${[r, g, b].map((v) => Math.round(v).toString(16).padStart(2, "0")).join("")}`;
}

function luminance([r, g, b]: Rgb): number {
  const lin = (v: number) => {
    const c = v / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

function darken(color: Rgb, amount: number): Rgb {
  return [color[0] * (1 - amount), color[1] * (1 - amount), color[2] * (1 - amount)];
}

/** WCAG contrast of white text on this colour. */
export function contrastWithWhite(color: string): number {
  return 1.05 / (luminance(rgb(color)) + 0.05);
}

/**
 * The colours behind an app's page and Fresh card: the tint, darkened until white text on it
 * reads at 4.5:1, and a deeper shade for the far end of the gradient.
 */
export function heroColors(color: string): { base: string; deep: string } {
  let base = rgb(color);
  for (let i = 0; i < 20 && 1.05 / (luminance(base) + 0.05) < 4.5; i++) base = darken(base, 0.06);
  return { base: hex(base), deep: hex(darken(base, 0.45)) };
}

/**
 * The tint when it is colourful enough to mark a progress ring or wash a card in both light
 * and dark mode; greys, near-whites and near-blacks give null (the accent is used instead).
 */
export function vividTint(tint: string | null | undefined): string | null {
  const parsed = parseTint(tint);
  if (!parsed) return null;
  const [r, g, b] = rgb(parsed).map((v) => v / 255) as Rgb;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const lightness = (max + min) / 2;
  const saturation = max === min ? 0 : (max - min) / (1 - Math.abs(2 * lightness - 1));
  return saturation >= 0.3 && lightness >= 0.2 && lightness <= 0.75 ? parsed : null;
}

/** How much of an install's signature is left (1 when just signed, 0 when expired), or null. */
export function signatureLeft(installed: Installed | null, now: Date = new Date()): number | null {
  if (!installed || installed.origin !== "kiwi") return null;
  const end = parseDate(installed.expires_at)?.getTime();
  const start = parseDate(installed.installed_at)?.getTime();
  if (end === undefined) return null;
  // Development profiles last a year; use that when the install time is missing or odd.
  const span = start !== undefined && end > start ? end - start : 365 * DAY;
  return Math.min(1, Math.max(0, (end - now.getTime()) / span));
}
