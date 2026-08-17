import {
  ClipboardIcon,
  StarIcon,
  CameraIcon,
  SettingsIcon,
  LaptopIcon,
  SunIcon,
  MoonIcon,
} from "./icons";
import type { DeviceIdentity } from "../types";
import { useUiStore, type AppTheme } from "../store";

export type NavTab = "history" | "favorites" | "capture" | "settings";

const TABS: Array<{ key: NavTab; label: string; shortcut: string; icon: (active: boolean) => React.ReactNode }> = [
  { key: "history", label: "剪贴板历史", shortcut: "⌘1", icon: () => <ClipboardIcon size={15} /> },
  { key: "favorites", label: "收藏夹", shortcut: "⌘2", icon: (active) => <StarIcon size={15} filled={active} /> },
  { key: "capture", label: "采集工作台", shortcut: "⌘3", icon: () => <CameraIcon size={15} /> },
  { key: "settings", label: "设置", shortcut: "⌘4", icon: () => <SettingsIcon size={15} /> },
];

const THEMES: Array<{ key: AppTheme; label: string; icon?: React.ReactNode }> = [
  { key: "light", label: "浅色", icon: <SunIcon size={13} /> },
  { key: "auto", label: "自动" },
  { key: "dark", label: "深色", icon: <MoonIcon size={13} /> },
];

interface SidebarProps {
  currentTab: NavTab;
  onSelectTab: (tab: NavTab) => void;
  identity?: DeviceIdentity | null;
}

export function Sidebar({ currentTab, onSelectTab, identity }: SidebarProps) {
  const { theme, setTheme } = useUiStore();

  return (
    <aside className="sidebar">
      <div className="brand">
        <div className="brand-mark">
          <img src="/asterism-app-icon.svg" alt="" />
        </div>
        <div>
          <div className="brand-name">Asterism</div>
          <div className="brand-sub">剪贴板 · 截图</div>
        </div>
      </div>

      <nav className="nav">
        {TABS.map((tab) => {
          const active = currentTab === tab.key;
          return (
            <button
              key={tab.key}
              className={`nav-item ${active ? "active" : ""}`}
              aria-current={active ? "page" : undefined}
              onClick={() => onSelectTab(tab.key)}
            >
              {tab.icon(active)}
              <span>{tab.label}</span>
              <kbd>{tab.shortcut}</kbd>
            </button>
          );
        })}
      </nav>

      <div className="sidebar-foot">
        <div className="segmented" role="group" aria-label="外观">
          {THEMES.map((option) => (
            <button
              key={option.key}
              aria-pressed={theme === option.key}
              onClick={() => setTheme(option.key)}
              title={option.key === "auto" ? "跟随系统" : option.label}
            >
              {option.icon}
              <span>{option.label}</span>
            </button>
          ))}
        </div>

        <div className="device-chip">
          <LaptopIcon size={15} style={{ color: "var(--text-tertiary)", flex: "0 0 auto" }} />
          <div style={{ minWidth: 0 }}>
            <div className="device-chip-name">{identity?.deviceName ?? "本机设备"}</div>
            <div className="device-chip-sub">
              <span className="status-dot" />
              <span>本地保险库就绪</span>
            </div>
          </div>
        </div>
      </div>
    </aside>
  );
}
