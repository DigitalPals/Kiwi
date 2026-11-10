import type { Tab, TabLink } from "../lib/routes";
import { Icon, type IconName } from "./Icon";

/** A tab whose screen has something to fix shows a dot in this tone. */
export type TabDots = Partial<Record<Tab, "warn" | "error">>;

const TAB_ICONS: Record<Tab, IconName> = {
  browse: "browse",
  apps: "apps",
  sources: "sources",
  builds: "wrench",
  settings: "settings",
};

export function TabBar(props: { tabs: readonly TabLink[]; active: Tab | null; updates: number; dots?: TabDots }) {
  const { tabs, active, updates, dots = {} } = props;
  return (
    <nav class="tab-bar" aria-label="Main">
      {tabs.map(({ tab, path, label }) => {
        const current = tab === active;
        const dot = dots[tab];
        return (
          <a key={tab} class={current ? "tab active" : "tab"} href={path} aria-current={current ? "page" : undefined}>
            <span class="tab-icon">
              <Icon name={TAB_ICONS[tab]} size={24} strokeWidth={current ? 2 : 1.7} />
              {tab === "apps" && updates > 0 ? (
                <span class="tab-badge" aria-label={`${updates} ${updates === 1 ? "update" : "updates"}`}>
                  {updates > 99 ? "99+" : updates}
                </span>
              ) : (
                dot && <span class={dot === "error" ? "tab-dot error" : "tab-dot"} aria-label="Needs attention" />
              )}
            </span>
            <span class="tab-label">{label}</span>
          </a>
        );
      })}
    </nav>
  );
}
