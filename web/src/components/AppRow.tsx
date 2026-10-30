import type { ComponentChildren } from "preact";
import type { AppState, AppSummary } from "../api/types";
import { replacesLabel, versionChange, type ButtonView } from "../lib/action";
import { expiryInfo } from "../lib/format";
import { appPath } from "../lib/routes";
import { signatureLeft, vividTint } from "../lib/tint";
import { ActionButton } from "./ActionButton";
import { AppIcon } from "./AppIcon";
import { VersionChange } from "./ui";

interface Props {
  app: AppSummary;
  onState: (app: AppSummary, state: AppState) => void;
  /** Second line; defaults to `RowDetail`. A running or failed action replaces it. */
  detail?: ComponentChildren;
  /** Replaces the action button (e.g. edit mode); `false` leaves the row without one. */
  trailing?: ComponentChildren;
  /** Extra content under the row (e.g. release notes). */
  children?: ComponentChildren;
  /** Orphaned installs have no detail page. */
  linkable?: boolean;
  /** Greyed out (e.g. hidden from Browse). */
  dimmed?: boolean;
  /** Replaces the icon, e.g. with `SignedIcon`. */
  icon?: ComponentChildren;
}

/** "Expires in 6 days", amber within a month and red within a week. */
export function ExpiryLabel({ expiresAt }: { expiresAt: string | null | undefined }) {
  const expiry = expiryInfo(expiresAt);
  if (!expiry) return <>Re-sign</>;
  return <span class={expiry.tone === "ok" ? undefined : `tone-${expiry.tone}`}>{expiry.label}</span>;
}

// The outline round a 40px icon in a 48px box, starting at the top centre and going clockwise.
const OUTLINE =
  "M24 1.2H33.8A13 13 0 0 1 46.8 14.2V33.8A13 13 0 0 1 33.8 46.8H14.2A13 13 0 0 1 1.2 33.8V14.2A13 13 0 0 1 14.2 1.2Z";

/**
 * The icon with a signing timer round it: the outline fills with the share of the signature
 * that is left, green, amber within a month, red within a week. Apps Kiwi didn't sign get no
 * timer but keep the same size, so the rows line up.
 */
export function SignedIcon({ app }: { app: AppSummary }) {
  const installed = app.state.installed;
  const left = signatureLeft(installed);
  const tone = expiryInfo(installed?.expires_at)?.tone ?? "ok";
  return (
    <span class={`signed-icon tone-${tone}`}>
      <AppIcon url={app.icon_url} name={app.name} tint={app.tint_color} size={40} />
      {left !== null && (
        <svg class="signed-ring" viewBox="0 0 48 48" aria-hidden="true">
          <path class="signed-track" d={OUTLINE} />
          {left > 0 && <path class="signed-fill" d={OUTLINE} pathLength={100} stroke-dasharray={`${Math.max(left * 100, 2)} 100`} />}
        </svg>
      )}
    </span>
  );
}

/**
 * The second line of a row: the expiry of a re-sign, the version an update installs, what
 * installing it replaces, why it can't be installed, or else what it is.
 */
export function RowDetail({ app }: { app: AppSummary }) {
  const { state } = app;
  if (state.action === "update" && state.update_reason === "resign") {
    return <ExpiryLabel expiresAt={state.installed?.expires_at} />;
  }
  const change = versionChange(state);
  if (change) return <VersionChange from={change.from} to={change.to} />;
  const replaces = replacesLabel(state);
  if (replaces) return <>{replaces}</>;
  if (state.action === "unavailable") return <>{app.unavailable ?? state.target?.incompatibility ?? "Unavailable"}</>;
  if (app.original) return <>Based on {app.original.short_name}</>;
  return <>{app.subtitle ?? app.developer ?? app.category ?? app.bundle_id}</>;
}

/** What a running or failed action says instead of the second line. */
function actionDetail(view: ButtonView): ComponentChildren {
  switch (view.kind) {
    case "progress":
      return `${view.caption ?? "Preparing"}…`;
    case "installing":
      return "Installing… confirm on your iPhone";
    case "prepare":
      return view.error ? <span class="tone-danger">Couldn't prepare{view.caption ? `: ${view.caption}` : ""}</span> : null;
    default:
      return null;
  }
}

export function AppRow({ app, onState, detail, trailing, children, linkable = true, dimmed = false, icon }: Props) {
  const top = (line: ComponentChildren, action: ComponentChildren) => {
    const info = (
      <>
        {icon ?? <AppIcon url={app.icon_url} name={app.name} tint={app.tint_color} size={48} />}
        <span class="app-row-text">
          <span class="app-row-name">{app.name}</span>
          <span class="app-row-detail">{line ?? detail ?? <RowDetail app={app} />}</span>
        </span>
      </>
    );
    return (
      <div class="app-row-top">
        {linkable ? (
          <a class="app-row-link" href={appPath(app.source_id, app.bundle_id)}>
            {info}
          </a>
        ) : (
          <div class="app-row-link">{info}</div>
        )}
        {action}
      </div>
    );
  };
  // Progress rings and update cards take the app's colour when it is a clear one.
  const tint = vividTint(app.tint_color);
  return (
    <li class={dimmed ? "app-row dimmed" : "app-row"} style={tint ? { "--app-tint": tint } : undefined}>
      {trailing != null ? (
        top(null, trailing)
      ) : (
        <ActionButton
          app={app}
          onState={onState}
          caption={false}
          render={(view, button) => top(actionDetail(view), button)}
        />
      )}
      {children}
    </li>
  );
}
