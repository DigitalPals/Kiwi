import { tabsFor, type Tab } from "../lib/routes";
import { Icon } from "./Icon";

/** A tab whose screen has something to fix shows a dot in this tone. */
export type TabDots = Partial<Record<Tab, "warn" | "error">>;

/** Members have three tabs (no Sources). */
export function TabBar(props: { active: Tab | null; updates: number; dots?: TabDots; member?: boolean }) {
  const { active, updates, dots = {}, member = false } = props;
  return (
    <nav class={member ? "tab-bar three" : "tab-bar"} aria-label="Main">
      {tabsFor(member).map(({ tab, path, label }) => {
        const current = tab === active;
        const dot = dots[tab];
        return (
          <a key={tab} class={current ? "tab active" : "tab"} href={path} aria-current={current ? "page" : undefined}>
            <span class="tab-icon">
              <Icon name={tab} size={24} strokeWidth={current ? 2 : 1.7} />
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
