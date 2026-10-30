import { useEffect, useLayoutEffect } from "preact/hooks";
import { api } from "./api/client";
import { TabBar } from "./components/TabBar";
import { Empty, Loading } from "./components/ui";
import { isMember } from "./lib/family";
import { useMe } from "./lib/me";
import { subscriptionBody, usableSubscription } from "./lib/push";
import { applyPendingScroll, previousTab, useRoute } from "./lib/router";
import { ownerOnly, routeTab, type Route } from "./lib/routes";
import { tabDots } from "./lib/warnings";
import { AppDetailScreen } from "./screens/AppDetail";
import { Browse } from "./screens/Browse";
import { MyApps } from "./screens/MyApps";
import { DeviceScreen, NotificationsScreen, Settings, SigningScreen } from "./screens/Settings";
import { SourceApps } from "./screens/SourceApps";
import { Sources } from "./screens/Sources";
import { Automation } from "./screens/Automation";
import { Builds } from "./screens/Builds";
import { AppleScreen, FamilyScreen } from "./screens/Family";
import { FamilyDeviceScreen, PersonScreen } from "./screens/FamilyPerson";
import { RegistrationGate } from "./screens/Register";

function Screen({ route, member, owner }: { route: Route; member: boolean; owner: string }) {
  if (member && ownerOnly(route)) {
    return (
      <main class="page">
        <Empty icon="settings" title="Not Available" message={`Only ${owner} can open this.`}>
          <a class="button" href="/">
            Browse Apps
          </a>
        </Empty>
      </main>
    );
  }
  switch (route.name) {
    case "browse":
      return <Browse />;
    case "apps":
      return <MyApps />;
    case "sources":
      return <Sources />;
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
    case "family":
      return <FamilyScreen />;
    case "person":
      return <PersonScreen key={route.personId} personId={route.personId} />;
    case "family-device":
      return <FamilyDeviceScreen key={route.deviceId} deviceId={route.deviceId} />;
    case "apple":
      return <AppleScreen />;
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
  const missing = me !== null && !me.push.subscribed && !!me.push.vapid_public_key;
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
  const { me, error, reload } = useMe();
  usePushResync();

  const path = location.pathname;
  useLayoutEffect(() => applyPendingScroll(), [path]);

  // Who is asking decides the screens (and which requests they may send), so wait for it.
  if (!me && !error) {
    return (
      <main class="page">
        <Loading />
      </main>
    );
  }
  const member = isMember(me);
  if (me && member && !me.registration.ready) return <RegistrationGate me={me} reload={reload} />;

  const active = routeTab(route) ?? (route.name === "app" ? previousTab() : null);
  return (
    <>
      <Screen route={route} member={member} owner={me?.access.owner_name ?? "the owner"} />
      <TabBar active={active} updates={me?.counts.updates ?? 0} dots={tabDots(me)} member={member} />
    </>
  );
}
