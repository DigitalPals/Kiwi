import { useEffect, useLayoutEffect } from "preact/hooks";
import { api, ApiError } from "./api/client";
import { TabBar } from "./components/TabBar";
import { Empty, Loading } from "./components/ui";
import { hasFeature, useFeatures } from "./lib/features";
import { useMe } from "./lib/me";
import { subscriptionBody, usableSubscription } from "./lib/push";
import { applyPendingScroll, navigate, previousTab, useRoute } from "./lib/router";
import { BUILDS_PATH, routeTab, shownTab, tabsFor, type Route } from "./lib/routes";
import { tabDots } from "./lib/warnings";
import { AppDetailScreen } from "./screens/AppDetail";
import { Browse } from "./screens/Browse";
import { MyApps } from "./screens/MyApps";
import { Onboarding } from "./screens/Onboarding";
import { DeviceScreen, NotificationsScreen, Settings, SigningScreen } from "./screens/Settings";
import { SourceApps } from "./screens/SourceApps";
import { Sources } from "./screens/Sources";
import { Automation } from "./screens/Automation";
import { Builds } from "./screens/Builds";

/** Replaces the address with `to`, e.g. for a page this device doesn't show. */
function Redirect({ to }: { to: string }) {
  useEffect(() => navigate(to, { replace: true }), [to]);
  return null;
}

function Screen({ route }: { route: Route }) {
  const { has } = useFeatures();
  switch (route.name) {
    case "browse":
      return <Browse />;
    case "apps":
      return <MyApps />;
    case "sources":
      // AltStore sources list iPhone apps: on Android, Kiwi Builds has the Sources tab's place.
      return has("android_apps") ? <Redirect to={BUILDS_PATH} /> : <Sources />;
    case "settings":
      return <Settings />;
    case "signing":
      return <SigningScreen />;
    case "device":
      return <DeviceScreen />;
    case "notifications":
      return <NotificationsScreen />;
    case "builds":
      return <Builds />;
    case "build":
      return <Automation key={route.app} app={route.app} />;
    case "source":
      return <SourceApps key={route.sourceId} sourceId={route.sourceId} />;
    case "app":
      return (
        <AppDetailScreen
          key={`${route.sourceId}/${route.bundleId}`}
          sourceId={route.sourceId}
          bundleId={route.bundleId}
        />
      );
    case "not-found":
      return (
        <main class="page">
          <Empty icon="warning" title="Page Not Found" message={route.path}>
            <a class="button" href="/">
              Browse Apps
            </a>
          </Empty>
        </main>
      );
  }
}

/** Re-registers this browser's push subscription if the server lost it (e.g. after a reset). */
function usePushResync() {
  const { me, reload } = useMe();
  const missing = me !== null && hasFeature(me, "web_push") && !me.push.subscribed && !!me.push.vapid_public_key;
  const key = me?.push.vapid_public_key ?? "";
  useEffect(() => {
    if (!missing || !("Notification" in window) || Notification.permission !== "granted") return;
    void usableSubscription(key)
      .then(async (sub) => {
        if (!sub) return;
        await api.subscribePush(subscriptionBody(sub));
        reload();
      })
      .catch(() => {});
  }, [missing, key, reload]);
}

export function App() {
  const route = useRoute();
  const { me, error } = useMe();
  usePushResync();

  const path = location.pathname;
  useLayoutEffect(() => applyPendingScroll(), [path]);

  // Kiwi is its owner's alone: wait to know whether this device is theirs.
  if (!me && !error) {
    return (
      <main class="page">
        <Loading />
      </main>
    );
  }
  if (error instanceof ApiError && error.code === "not_owner") {
    return (
      <main class="page">
        <Empty icon="shield" title="Not Your Kiwi" message={error.message} />
      </main>
    );
  }

  // A new device first says what it is; the tabs follow from that.
  if (me && !me.onboarded) return <Onboarding me={me} />;

  const tabs = tabsFor(me?.features ?? []);
  const active = shownTab(routeTab(route) ?? (route.name === "app" ? previousTab() : null), tabs);
  return (
    <>
      <Screen route={route} />
      <TabBar tabs={tabs} active={active} updates={me?.counts.updates ?? 0} dots={tabDots(me)} />
    </>
  );
}
