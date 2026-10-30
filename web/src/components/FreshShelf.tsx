import { useRef } from "preact/hooks";
import type { AppState, AppSummary } from "../api/types";
import { versionChange } from "../lib/action";
import { freshApps, freshLabel } from "../lib/fresh";
import { appPath } from "../lib/routes";
import { appColor, heroColors } from "../lib/tint";
import { ActionButton } from "./ActionButton";
import { AppIcon } from "./AppIcon";

function FreshCard({ app, onState }: { app: AppSummary; onState: (app: AppSummary, state: AppState) => void }) {
  const { base, deep } = heroColors(appColor(app.name, app.tint_color));
  const change = versionChange(app.state);
  return (
    <li class="fresh-card" style={{ "--hero": base, "--hero-deep": deep }}>
      <a class="fresh-link" href={appPath(app.source_id, app.bundle_id)}>
        <AppIcon url={app.icon_url} name={app.name} tint={app.tint_color} size={44} />
        <span class="fresh-label">{freshLabel(app)}</span>
        <span class="fresh-name">{app.name}</span>
        <span class="fresh-detail">
          {change ? `${change.from} → ${change.to}` : (app.subtitle ?? app.developer ?? app.source_name)}
        </span>
      </a>
      <div class="fresh-action">
        <ActionButton app={app} onState={onState} caption={false} />
      </div>
    </li>
  );
}

const appKey = (app: AppSummary) => `${app.source_id}/${app.bundle_id}`;

/**
 * Browse's "Fresh This Week": big cards in each app's own colour (see `freshApps`). The cards
 * are picked once from the first list, so a card stays put while its app installs; remount
 * (e.g. with a `key`) to pick again.
 */
export function FreshShelf({ apps, onState }: { apps: AppSummary[]; onState: (app: AppSummary, state: AppState) => void }) {
  const picked = useRef<string[] | null>(null);
  if (picked.current === null && apps.length > 0) picked.current = freshApps(apps).map(appKey);
  const byKey = new Map(apps.map((app) => [appKey(app), app]));
  const cards = (picked.current ?? []).flatMap((key) => byKey.get(key) ?? []);
  if (cards.length === 0) return null;
  return (
    <section class="fresh" aria-labelledby="fresh-title">
      <div class="list-header">
        <h2 class="list-title" id="fresh-title">
          Fresh This Week
        </h2>
      </div>
      <ul class="fresh-shelf">
        {cards.map((app) => (
          <FreshCard key={appKey(app)} app={app} onState={onState} />
        ))}
      </ul>
    </section>
  );
}
