import { readFileSync } from "node:fs";
import type { AutomationPreferences, AutomationState, BuildFeature } from "../src/api/types.ts";

interface RecipePackage {
  id: string;
  optional?: boolean;
  default?: boolean;
  name?: string;
  description?: string;
}

/** What the built-in Kiwi Builds source lists for an app; `{name}` is the chosen app name. */
export interface RecipeCatalog {
  name: string;
  developer: string;
  subtitle: string;
  description: string;
  tint_color: string;
  category: string;
}

interface Recipe {
  id: string;
  name: string;
  bundle_id: string;
  tweak: string;
  summary: string;
  experimental?: boolean;
  warning?: string;
  names?: string[];
  app_version?: string;
  native?: { version: string; build: string };
  integration?: { build: string };
  interval_minutes: number;
  /** Releases the source keeps. */
  retain: number;
  catalog: RecipeCatalog;
  limitations: AutomationState["limitations"];
  packages: RecipePackage[];
}

/** The server's own recipes, so the mock offers the same apps and options. */
const recipes: Recipe[] = JSON.parse(readFileSync(new URL("../../automation/recipes.json", import.meta.url), "utf8")).apps;

export const nativeBuildOf = (id: string) => recipes.find((r) => r.id === id)?.native;
export const integrationOf = (id: string) => recipes.find((r) => r.id === id)?.integration;

const features = (packages: RecipePackage[]): BuildFeature[] =>
  packages.flatMap((p) => (p.name ? [{ id: p.id, name: p.name, description: p.description ?? "" }] : []));

export function defaultPreferences(app: AutomationState): AutomationPreferences {
  const recipe = recipes.find((r) => r.id === app.id)!;
  return {
    enabled: true,
    interval_minutes: recipe.interval_minutes,
    tweaks: recipe.packages.filter((p) => p.optional && p.default).map((p) => p.id).sort(),
    ...(recipe.names?.length ? { name: recipe.names[0] } : {}),
  };
}

/** Kiwi Builds as a new Kiwi starts: YouTube added, the other apps available to add. */
export function createBuilds(now: string): AutomationState[] {
  return recipes.map((r) => {
    const app: AutomationState = {
      id: r.id, name: r.name, bundle_id: r.bundle_id, icon_url: null, tweak: r.tweak, summary: r.summary,
      experimental: !!r.experimental, warning: r.warning ?? null, names: r.names ?? [], app_version: r.app_version ?? null,
      limitations: r.limitations,
      included: features(r.packages.filter((p) => !p.optional)),
      available_tweaks: features(r.packages.filter((p) => p.optional)),
      added: r.id === "youtube",
      preferences: { enabled: true, interval_minutes: r.interval_minutes, tweaks: [] },
      jobs: [], checks: [], last_checked_at: null, last_success_key: null, failed_recipe: null, failed_attempts: 0,
      sequence: 0, published: null, source_enabled: null, configuration_changed: false,
      next_check_at: r.id === "youtube" ? now : null,
    };
    app.preferences = defaultPreferences(app);
    return app;
  });
}

/** The source entry of an app built with `name` (Kiwi's `Recipe::catalog_name`). */
export function catalogOf(id: string, name?: string): RecipeCatalog & { retain: number } {
  const recipe = recipes.find((r) => r.id === id)!;
  const chosen = name ?? recipe.names?.[0] ?? recipe.name;
  return { ...recipe.catalog, name: recipe.catalog.name.replace("{name}", chosen), retain: recipe.retain };
}

/** The stages the Mac reports while it builds an app, in the runner's order (cached sources). */
export function buildStages(app: AutomationState): string[] {
  if (nativeBuildOf(app.id)) return [
    "Checking native iOS build tools",
    "Downloading pinned Rambler caller metadata",
    "Extracting private Rambler caller configuration",
    "Testing and compiling the native Rambler client",
    "Verifying Rambler speech corrections",
    "Building the Rambler iPhone app",
    "Validating native iOS executables",
    "Signing and verifying with Kiwi",
  ];
  if (integrationOf(app.id)) return [
    "Checking build setup",
    "Checking the App Store",
    `Downloading original ${app.name}`,
    `Decrypting ${app.name}`,
    "Extracting private Rambler caller configuration",
    "Testing and compiling the native Rambler client",
    "Verifying Rambler speech corrections",
    "Compiling native Gboard Rambler hooks",
    "Integrating Rambler and preserving Gboard's keyboard",
    "Validating patched executables",
    "Signing and verifying with Kiwi",
  ];
  return [
    "Checking build setup",
    "Checking the App Store",
    `Compiling ${app.tweak.split(" ")[0]} and selected tweaks`,
    `Downloading original ${app.name}`,
    `Decrypting ${app.name}`,
    "Injecting tweaks",
    "Validating patched executables",
    "Signing and verifying with Kiwi",
  ];
}
