import { readFileSync } from "node:fs";
import type { AutomationPreferences, AutomationState, BuildFeature } from "../src/api/types.ts";

interface RecipePackage {
  id: string;
  optional?: boolean;
  default?: boolean;
  name?: string;
  description?: string;
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
  interval_minutes: number;
  limitations: AutomationState["limitations"];
  packages: RecipePackage[];
}

/** The server's own recipes, so the mock offers the same apps and options. */
const recipes: Recipe[] = JSON.parse(readFileSync(new URL("../../automation/recipes.json", import.meta.url), "utf8")).apps;

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
