import type { JSX } from "preact";
import { useCallback, useMemo } from "preact/hooks";
import type { AppState, AppSummary, Release } from "../api/types";
import { deriveAppButton, deriveVersionButton, isInstalledRelease, type ButtonView } from "../lib/action";
import { formatPercent } from "../lib/format";
import { useInstallFlow } from "../lib/install-flow";
import { Spinner } from "./ui";

interface ViewProps {
  view: ButtonView;
  onPrepare: () => void;
  onInstallTap: (href: string) => void;
  large?: boolean | undefined;
  /** Show the caption under the pill (version history) or let the parent show it (rows, app page). */
  caption?: boolean | undefined;
}

function ProgressRing({ progress }: { progress: number }) {
  const r = 12;
  const c = 2 * Math.PI * r;
  return (
    <span
      class="ring"
      role="progressbar"
      aria-label="Preparing"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(progress * 100)}
      title={formatPercent(progress) ?? undefined}
    >
      <svg viewBox="0 0 30 30" width="30" height="30" aria-hidden="true">
        <circle class="ring-track" cx="15" cy="15" r={r} />
        <circle class="ring-bar" cx="15" cy="15" r={r} stroke-dasharray={c} stroke-dashoffset={c * (1 - progress)} />
      </svg>
    </span>
  );
}

export function ActionView({ view, onPrepare, onInstallTap, large, caption = true }: ViewProps) {
  const size = large ? " pill-lg" : "";
  let pill;
  let text: string | null = null;
  let error = false;

  switch (view.kind) {
    case "prepare":
      pill = (
        <button type="button" class={`pill${size}`} onClick={onPrepare}>
          {view.label}
        </button>
      );
      // Rows show the error on their second line; captions are for other lists.
      text = view.error ? view.caption : null;
      error = view.error;
      break;
    case "progress":
      pill =
        view.progress === null ? (
          <span class={`pill pill-muted${size}`} role="status">
            <Spinner /> Preparing
          </span>
        ) : (
          <ProgressRing progress={view.progress} />
        );
      text = view.caption;
      break;
    case "link":
      // A real link: iOS only starts an OTA install from a direct tap on itms-services.
      pill = (
        <a class={`pill pill-filled${size}`} href={view.href} onClick={() => onInstallTap(view.href)}>
          {view.label}
        </a>
      );
      break;
    case "installing":
      // Still a link so the install prompt can be reopened if it was dismissed.
      pill = view.href ? (
        <a class={`pill pill-muted${size}`} href={view.href} onClick={() => onInstallTap(view.href ?? "")}>
          <Spinner /> Installing
        </a>
      ) : (
        <span class={`pill pill-muted${size}`} role="status">
          <Spinner /> Installing
        </span>
      );
      break;
    case "open":
      pill = (
        <a class={`pill${size}`} href={view.href}>
          Open
        </a>
      );
      break;
    case "disabled":
      pill = (
        <span class={`pill pill-disabled${size}`} aria-disabled="true">
          {view.label}
        </span>
      );
      break;
  }

  return (
    <div class="action">
      {pill}
      {caption && text && (
        <span class={error ? "action-caption danger" : "action-caption"} title={text}>
          {text}
        </span>
      )}
    </div>
  );
}

interface ActionButtonProps {
  app: AppSummary;
  onState: (app: AppSummary, state: AppState) => void;
  large?: boolean;
  caption?: boolean;
  /** Receives the derived view, e.g. to show the full error or step elsewhere. */
  render?: (view: ButtonView, button: JSX.Element) => JSX.Element;
}

export function ActionButton({ app, onState, large, caption, render }: ActionButtonProps) {
  const handleState = useCallback((state: AppState) => onState(app, state), [app, onState]);
  const { local, prepare, installTapped } = useInstallFlow({ app, onState: handleState, mode: "app" });
  const view = deriveAppButton(app, local);
  const button = (
    <ActionView view={view} onPrepare={prepare} onInstallTap={installTapped} large={large} caption={caption} />
  );
  return render ? render(view, button) : button;
}

interface VersionButtonProps {
  app: AppSummary;
  release: Release;
  onState: (app: AppSummary, state: AppState) => void;
}

export function VersionButton({ app, release, onState }: VersionButtonProps) {
  const handleState = useCallback((state: AppState) => onState(app, state), [app, onState]);
  const prepareBody = useMemo(
    () => ({ version: release.version, ...(release.build ? { build: release.build } : {}),
      ...(release.revision !== undefined ? { revision: release.revision } : {}) }),
    [release],
  );
  const isDone = useCallback((state: AppState) => isInstalledRelease(release, state.installed), [release]);
  const { local, prepare, installTapped } = useInstallFlow({
    app,
    onState: handleState,
    mode: "version",
    prepareBody,
    isDone,
  });
  const view = deriveVersionButton(release, app.state.installed, local);
  if (view.kind === "prepare" && !view.error) {
    return (
      <button type="button" class="text-button" onClick={prepare}>
        Install this version
      </button>
    );
  }
  return <ActionView view={view} onPrepare={prepare} onInstallTap={installTapped} />;
}
