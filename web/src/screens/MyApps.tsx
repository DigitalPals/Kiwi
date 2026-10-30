import { useCallback, useState } from "preact/hooks";
import { api, errorMessage } from "../api/client";
import type { AppState, AppSummary } from "../api/types";
import { AppRow, ExpiryLabel, SignedIcon } from "../components/AppRow";
import { Icon } from "../components/Icon";
import { Banner, Empty, ErrorState, ExpandableText, Page, SkeletonRows, Spinner, VersionChange } from "../components/ui";
import { libraryKey, needsPrepare, versionChange, withAppState, withLibraryState } from "../lib/action";
import { expiryInfo, formatBytes, formatDate, formatRelative, formatVersion } from "../lib/format";
import { useOnForeground, useResource, useTask } from "../lib/hooks";
import { useMe, useOnUpdateCountChange } from "../lib/me";

/** "21.43.5 → 21.44.0 · 152.4 MB", or when a re-sign's current build expires. */
function UpdateDetail({ app }: { app: AppSummary }) {
  const { installed, target, build, update_reason } = app.state;
  if (update_reason === "resign" || (installed && target && installed.version === target.version)) {
    return <ExpiryLabel expiresAt={installed?.expires_at} />;
  }
  const change = versionChange(app.state);
  const size = formatBytes(build?.size ?? target?.size ?? null);
  return (
    <>
      {change ? <VersionChange from={change.from} to={change.to} /> : target ? `Version ${formatVersion(target)}` : app.source_name}
      {size !== "—" && ` · ${size}`}
    </>
  );
}

/** The version, then where the install came from or until when it is signed. */
function InstalledDetail({ app }: { app: AppSummary }) {
  const installed = app.state.installed;
  if (!installed) return <>{app.source_name}</>;
  const version = installed.version;
  if (app.orphaned) {
    return <>{version} · {app.source_name === "Removed source" ? "source removed" : "no longer offered"}</>;
  }
  if (installed.origin === "device") return <>{version} · installed outside Kiwi</>;
  const expiry = expiryInfo(installed.expires_at);
  if (!expiry) return <>{version}</>;
  if (expiry.tone === "ok") return <>{version} · signed until {formatDate(installed.expires_at)}</>;
  return (
    <>
      {version} · <ExpiryLabel expiresAt={installed.expires_at} />
    </>
  );
}

function lastChecked(sources: { enabled: boolean; last_attempt_at: string | null }[] | undefined): string | null {
  const times = (sources ?? []).filter((s) => s.enabled && s.last_attempt_at).map((s) => s.last_attempt_at as string);
  return times.length ? times.reduce((a, b) => (a > b ? a : b)) : null;
}

export function MyApps() {
  const updates = useResource("updates", api.updates);
  const library = useResource("library", api.library);
  const sources = useResource("sources", api.sources);
  const { reload: reloadMe } = useMe();
  const [editing, setEditing] = useState(false);
  const [forgetError, setForgetError] = useState<string | null>(null);
  const [forgetting, setForgetting] = useState<string | null>(null);

  const mutateUpdates = updates.mutate;
  const mutateLibrary = library.mutate;
  // An update appears in both lists; keep their states in step. A finished update moves to
  // Installed, which remounts its row, so the row can't notice the change itself: refresh the
  // counts (tab badge) here.
  const onState = useCallback(
    (app: AppSummary, state: AppState) => {
      mutateUpdates((list) => list && withAppState(list, app, state));
      mutateLibrary((list) => list && withLibraryState(list, app, state));
      if (state.action !== app.state.action) reloadMe();
    },
    [mutateUpdates, mutateLibrary, reloadMe],
  );

  const reloadAll = useCallback(() => {
    void updates.reload();
    void library.reload();
  }, [updates.reload, library.reload]);
  useOnForeground(reloadAll);
  useOnUpdateCountChange(reloadAll);

  const prepareAll = useTask(async () => {
    await api.prepareUpdates();
    await updates.reload();
    reloadMe();
  });
  const check = useTask(async () => {
    sources.mutate(await api.refreshSources());
    await Promise.all([updates.reload(), library.reload()]);
    reloadMe();
  });

  const forget = async (app: AppSummary) => {
    const signedId = app.state.installed?.signed_bundle_id;
    if (!signedId) return;
    const ok = window.confirm(
      `Forget ${app.name}?\n\nKiwi stops tracking it. The app stays on your iPhone until you delete it there.`,
    );
    if (!ok) return;
    setForgetError(null);
    setForgetting(signedId);
    try {
      await api.forget(signedId);
      mutateLibrary((list) => list?.filter((a) => libraryKey(a) !== signedId));
      mutateUpdates((list) => list?.filter((a) => libraryKey(a) !== signedId));
      reloadMe();
    } catch (error) {
      setForgetError(errorMessage(error));
    } finally {
      setForgetting(null);
    }
  };

  const forgetButton = (app: AppSummary) =>
    editing && app.state.installed ? (
      <button
        type="button"
        class="pill pill-danger"
        onClick={() => void forget(app)}
        disabled={forgetting === libraryKey(app)}
      >
        Forget
      </button>
    ) : undefined;

  // Updates stay listed until they are installed (action "open"); then they move down.
  const pending = updates.data?.filter((a) => a.state.action !== "open");
  const pendingKeys = new Set(pending?.map(libraryKey));
  const installed = library.data?.filter((a) => !pendingKeys.has(libraryKey(a)));
  const ready = pending?.some((a) => a.state.install_url && a.state.action === "update");

  let content;
  if (!pending && !installed) {
    const error = updates.error ?? library.error;
    content = error ? <ErrorState error={error} onRetry={reloadAll} /> : <SkeletonRows count={4} />;
  } else if (pending?.length === 0 && installed?.length === 0) {
    content = (
      <Empty icon="apps" title="No Apps Yet" message="Apps you install with Kiwi appear here, with their updates.">
        <a class="button" href="/">
          Browse Apps
        </a>
      </Empty>
    );
  } else {
    content = (
      <>
        <div class="list-header">
          <h2 class="list-title">
            Updates{pending && pending.length > 0 && <span class="count">{pending.length}</span>}
          </h2>
          {pending?.some((a) => needsPrepare(a.state)) && !editing && (
            <button
              type="button"
              class="soft-button"
              onClick={() => void prepareAll.run()}
              disabled={prepareAll.pending}
            >
              {prepareAll.pending ? <Spinner /> : "Update All"}
            </button>
          )}
        </div>
        {prepareAll.error && <Banner level="error">{prepareAll.error}</Banner>}
        {!pending ? (
          updates.error ? (
            <Banner level="error">Couldn't load updates: {updates.error.message}</Banner>
          ) : (
            <SkeletonRows count={1} />
          )
        ) : pending.length === 0 ? (
          <div class="card up-to-date">
            <Icon name="check" size={18} strokeWidth={2.2} />
            All apps are up to date
          </div>
        ) : (
          <>
            <ul class="app-list update-cards">
              {pending.map((app) => (
                <AppRow
                  key={libraryKey(app)}
                  app={app}
                  onState={onState}
                  icon={<SignedIcon app={app} />}
                  detail={<UpdateDetail app={app} />}
                  trailing={forgetButton(app)}
                >
                  {app.state.target?.notes && app.state.update_reason !== "resign" && (
                    <div class="row-notes">
                      <ExpandableText text={app.state.target.notes} lines={2} />
                    </div>
                  )}
                </AppRow>
              ))}
            </ul>
            {ready && <p class="footnote">iOS asks you to confirm each install, so tap Update on every app that's ready.</p>}
          </>
        )}

        {installed && installed.length > 0 && (
          <>
            <div class="list-header">
              <h2 class="list-title">
                Installed<span class="count">{installed.length}</span>
              </h2>
            </div>
            <ul class="app-list">
              {installed.map((app) => (
                <AppRow
                  key={libraryKey(app)}
                  app={app}
                  onState={onState}
                  linkable={!app.orphaned}
                  icon={<SignedIcon app={app} />}
                  detail={<InstalledDetail app={app} />}
                  // Everything here is installed: a button only when Kiwi can open the app.
                  trailing={forgetButton(app) ?? (app.state.action === "open" && !app.state.open_url ? false : undefined)}
                />
              ))}
            </ul>
          </>
        )}
        {!installed && library.error && <Banner level="error">Couldn't load installed apps: {library.error.message}</Banner>}
        {editing && <p class="footnote">Forget stops tracking an app. It stays on your iPhone until you delete it there.</p>}
      </>
    );
  }

  const checked = lastChecked(sources.data);
  const hasApps = (pending?.length ?? 0) + (installed?.length ?? 0) > 0;
  return (
    <Page
      title="My Apps"
      actions={
        hasApps && (
          <button type="button" class="nav-button" onClick={() => setEditing(!editing)}>
            {editing ? "Done" : "Edit"}
          </button>
        )
      }
      summary={
        <>
          <span>{checked ? `Checked ${formatRelative(checked)}` : "Not checked yet"}</span>
          <span>·</span>
          <button type="button" class="text-button small" onClick={() => void check.run()} disabled={check.pending}>
            {check.pending ? "Checking…" : "Check Now"}
          </button>
          {check.error && <span class="danger">{check.error}</span>}
        </>
      }
    >
      {forgetError && <Banner level="error">{forgetError}</Banner>}
      {updates.data && library.data && (updates.error || library.error) && (
        <Banner level="error">Couldn't refresh: {(updates.error ?? library.error)?.message}</Banner>
      )}
      {content}
    </Page>
  );
}
