import { useCallback, useMemo, useState } from "preact/hooks";
import { api } from "../api/client";
import type { AndroidApp, AppState, AppSummary, Me } from "../api/types";
import { KiwiAppIcon } from "../components/AppIcon";
import { AppRow } from "../components/AppRow";
import { FreshShelf } from "../components/FreshShelf";
import { Icon } from "../components/Icon";
import { KiwiLogo } from "../components/KiwiLogo";
import { Banner, Empty, ErrorState, MenuPicker, Page, SkeletonRows, Spinner } from "../components/ui";
import { withAppState } from "../lib/action";
import { plural } from "../lib/format";
import { BUILDS_PATH } from "../lib/routes";
import { hasFeature } from "../lib/features";
import { useDebounced, useResource, useStoredFlag, useTask } from "../lib/hooks";
import { useMe } from "../lib/me";
import { shouldSuggestHomeScreen } from "../lib/pwa";
import { blockingWarnings } from "../lib/warnings";

// Survive navigation to an app page and back.
let savedQuery = "";
let savedSource: string | null = null;
let savedShowHidden = false;

/** Puts a hidden app back in Browse. */
function ShowButton({ app, onShown }: { app: AppSummary; onShown: () => void }) {
  const show = useTask(async () => {
    await api.updateApp(app.source_id, app.bundle_id, { hidden: false });
    onShown();
  });
  return (
    <div class="action">
      <button
        type="button"
        class="pill"
        onClick={() => void show.run()}
        disabled={show.pending}
        aria-label={`Show ${app.name} in Browse`}
      >
        {show.pending ? <Spinner /> : "Show"}
      </button>
      {show.error && <span class="action-caption danger">{show.error}</span>}
    </div>
  );
}

/** Only problems that stop every install; the rest show where they are fixed. */
function BlockingWarnings({ me }: { me: Me }) {
  return (
    <>
      {blockingWarnings(me).map((w) => (
        <Banner key={`${w.code}:${w.message}`} level={w.level} href={w.link}>
          {w.message}
        </Banner>
      ))}
    </>
  );
}

function HomeScreenHint() {
  const [dismissed, setDismissed] = useStoredFlag("kiwi.homeScreenHintDismissed");
  if (dismissed || !shouldSuggestHomeScreen()) return null;
  return (
    <Banner level="info" icon="share" onDismiss={() => setDismissed(true)}>
      <strong>Add to Home Screen</strong> for notifications and badges: tap Share, then “Add to Home Screen”.
    </Banner>
  );
}

/** Android in a browser: Kiwi's own app, which installs updates in the background. */
function KiwiAppOffer({ app }: { app: AndroidApp }) {
  const [dismissed, setDismissed] = useStoredFlag("kiwi.androidAppOfferDismissed");
  if (dismissed) return null;
  return (
    <div class="app-offer">
      <KiwiAppIcon size={40} />
      <div class="cell-main">
        <div class="app-offer-name">Get the Kiwi app</div>
        <div class="app-offer-detail">Installs updates for you</div>
      </div>
      <a class="pill" href={app.install_url} aria-label={`Get Kiwi for Android ${app.version_name}`}>
        Get
      </a>
      <button type="button" class="banner-close" onClick={() => setDismissed(true)} aria-label="Dismiss">
        <Icon name="close" size={16} />
      </button>
    </div>
  );
}

function byRecentRelease(a: AppSummary, b: AppSummary): number {
  const da = a.state.target?.date ?? "";
  const db = b.state.target?.date ?? "";
  return db.localeCompare(da) || a.name.localeCompare(b.name);
}

function SearchField({ query, onInput }: { query: string; onInput: (value: string) => void }) {
  return (
    <label class="search">
      <Icon name="search" size={18} />
      <input
        type="search"
        placeholder="Apps and developers"
        aria-label="Search apps"
        value={query}
        onInput={(e) => onInput(e.currentTarget.value)}
        enterKeyHint="search"
        autocapitalize="off"
        autocorrect="off"
        spellcheck={false}
      />
    </label>
  );
}

const SORTS = [
  { value: "name", label: "Name" },
  { value: "recent", label: "Recently Updated" },
] as const;

export function Browse() {
  const { me } = useMe();
  // Android apps all come from Kiwi Builds: no source to pick.
  const android = hasFeature(me, "android_apps");
  const [query, setQuery] = useState(savedQuery);
  const [savedPick, setSource] = useState(savedSource);
  const source = android ? null : savedPick;
  const [showHidden, setShowHidden] = useState(savedShowHidden);
  const [recent, setRecent] = useStoredFlag("kiwi.browseSortRecent");
  const q = useDebounced(query.trim(), 250);
  // Searches find hidden apps too, listed after the others. Browse always asks for the hidden
  // apps and counts them from the list: a source's own count includes other platforms' apps
  // (Kiwi Builds' source has both), so an iPhone would count hidden Android apps it never lists.
  const listHidden = showHidden || !!q;

  const sources = useResource("sources", api.sources);
  const apps = useResource(`apps?q=${q}&source=${source ?? ""}&hidden=true`, () =>
    api.apps({ q, ...(source ? { source } : {}), hidden: true }),
  );
  const { mutate } = apps;
  const onState = useCallback(
    (app: AppSummary, state: AppState) => mutate((list) => list && withAppState(list, app, state)),
    [mutate],
  );

  const enabledSources = sources.data?.filter((s) => s.enabled) ?? [];
  const chooseSource = (id: string | null) => {
    savedSource = id;
    setSource(id);
  };
  const toggleHidden = () => {
    savedShowHidden = !showHidden;
    setShowHidden(savedShowHidden);
  };
  const reloadAll = () => {
    void apps.reload();
    void sources.reload();
  };

  let content;
  // With hidden apps the server lists every platform's (a source's page shows them all);
  // Browse keeps to this device's.
  const platform = android ? "android" : hasFeature(me, "ios_apps") ? "ios" : null;
  const { shown, hidden } = useMemo(() => {
    const shown: AppSummary[] = [];
    const hidden: AppSummary[] = [];
    for (const app of apps.data ?? []) {
      if (platform && app.platform !== platform) continue;
      (app.hidden ? hidden : shown).push(app);
    }
    if (recent) {
      shown.sort(byRecentRelease);
      hidden.sort(byRecentRelease);
    }
    return { shown, hidden };
  }, [apps.data, recent, platform]);
  const hiddenToggle =
    !q && hidden.length > 0 ? (
      <button type="button" class="hidden-toggle" onClick={toggleHidden} aria-expanded={showHidden}>
        {plural(hidden.length, "app")} hidden · <b>{showHidden ? "Hide" : "Show"}</b>
      </button>
    ) : null;
  // Reuse the list while the search input is changing but its debounced results have
  // not. Preact can retain these rows without rendering every action button again.
  const shownRows = useMemo(() => shown.map((app) => (
    <AppRow key={`${app.source_id}/${app.bundle_id}`} app={app} onState={onState} />
  )), [shown, onState]);
  const hiddenList =
    listHidden && hidden.length > 0 ? (
      <>
        <div class="list-header">
          <h2 class="list-title">
            Hidden<span class="count">{hidden.length}</span>
          </h2>
        </div>
        <ul class="app-list">
          {hidden.map((app) => (
            <AppRow
              key={`${app.source_id}/${app.bundle_id}`}
              app={app}
              onState={onState}
              dimmed
              trailing={<ShowButton app={app} onShown={reloadAll} />}
            />
          ))}
        </ul>
      </>
    ) : null;
  if (!apps.data) {
    content = apps.error ? <ErrorState error={apps.error} onRetry={apps.reload} /> : <SkeletonRows />;
  } else if (shown.length === 0 && hidden.length > 0) {
    content = (
      <>
        {q ? (
          <Empty icon="search" title="No Results" message={`Only hidden apps match “${q}”.`} />
        ) : (
          <Empty
            icon="eyeOff"
            title="No Apps in Browse"
            message={`Every app here is hidden. Pick the ones you want in ${android ? "Kiwi Builds" : "Sources"}.`}
          >
            <a class="button" href={android ? BUILDS_PATH : "/sources"}>
              Choose Apps
            </a>
          </Empty>
        )}
        {hiddenToggle}
        {hiddenList}
      </>
    );
  } else if (shown.length + hidden.length === 0) {
    if (android && !q) {
      // AltStore sources list iPhone apps; an Android device installs what Kiwi Builds makes.
      content = (
        <Empty
          icon="wrench"
          art={<KiwiLogo size={72} />}
          title="No Android Apps Yet"
          message="Kiwi Builds makes Android apps for you. Add one there and it shows up here."
        >
          <a class="button" href={BUILDS_PATH}>
            Open Kiwi Builds
          </a>
        </Empty>
      );
    } else if (!android && sources.data && enabledSources.length === 0) {
      content = (
        <Empty
          icon="sources"
          art={<KiwiLogo size={72} />}
          title="Welcome to Kiwi"
          message="Add an AltStore source to browse its apps and install them on your iPhone."
        >
          <a class="button" href="/sources">
            Add a Source
          </a>
        </Empty>
      );
    } else if (q) {
      content = <Empty icon="search" title="No Results" message={`Nothing matches “${q}”.`} />;
    } else {
      content = <Empty icon="inbox" title="No Apps" message="Your sources don't list any apps yet." />;
    }
  } else {
    content = (
      <>
        {apps.error && <Banner level="error">Couldn't refresh: {apps.error.message}</Banner>}
        <ul class="app-list">{shownRows}</ul>
        {hiddenToggle}
        {hiddenList}
        {android && !q && (
          <p class="fine-print browse-note">Android apps come from Kiwi Builds. Sources list iPhone apps, so they stay hidden here.</p>
        )}
      </>
    );
  }

  const sourceOptions = [
    { value: "", label: "All Sources" },
    ...enabledSources.map((s) => ({ value: s.id, label: s.name })),
  ];
  return (
    <Page title="Browse">
      {hasFeature(me, "android_app_offer") && me?.android_app && <KiwiAppOffer app={me.android_app} />}
      {me && <BlockingWarnings me={me} />}
      {hasFeature(me, "home_screen_app") && <HomeScreenHint />}
      <SearchField
        query={query}
        onInput={(value) => {
          savedQuery = value;
          setQuery(value);
        }}
      />
      {!q && <FreshShelf key={source ?? ""} apps={shown} onState={onState} />}
      {apps.data && (shown.length > 0 || (listHidden && hidden.length > 0) || source !== null || q) && (
        <div class="list-header">
          <h2 class="list-title">
            {q ? "Results" : "All Apps"}
            <span class="count">{shown.length}</span>
          </h2>
          <span class="filter-chips">
            {!android && enabledSources.length > 1 && (
              <MenuPicker
                label="Show apps from"
                value={source ?? ""}
                options={sourceOptions}
                onChange={(id) => chooseSource(id || null)}
              />
            )}
            <MenuPicker
              label="Sort apps"
              value={recent ? "recent" : "name"}
              options={SORTS}
              onChange={(value) => setRecent(value === "recent")}
            />
          </span>
        </div>
      )}
      {content}
    </Page>
  );
}
