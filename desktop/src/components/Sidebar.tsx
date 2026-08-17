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
import { useUiStore } from "../store";

export type NavTab = "history" | "favorites" | "capture" | "settings";

interface SidebarProps {
  currentTab: NavTab;
  onSelectTab: (tab: NavTab) => void;
  identity?: DeviceIdentity | null;
}

export function Sidebar({
  currentTab,
  onSelectTab,
  identity,
}: SidebarProps) {
  const { theme, setTheme } = useUiStore();

  return (
    <aside className="sidebar">
      <div>
        <div className="brand-section">
          <div className="brand-icon">
            <img src="/asterism-app-icon.svg" alt="" />
          </div>
          <div className="brand-title">
            <span>Asterism</span>
            <span className="brand-badge">PRO</span>
          </div>
        </div>

        <nav className="nav-menu" style={{ marginTop: 14 }}>
          <button
            className={`nav-item ${currentTab === "history" ? "active" : ""}`}
            onClick={() => onSelectTab("history")}
          >
            <div className="nav-item-left">
              <ClipboardIcon size={17} />
              <span>剪贴板历史</span>
            </div>
          </button>

          <button
            className={`nav-item ${currentTab === "favorites" ? "active" : ""}`}
            onClick={() => onSelectTab("favorites")}
          >
            <div className="nav-item-left">
              <StarIcon size={17} filled={currentTab === "favorites"} />
              <span>收藏夹</span>
            </div>
          </button>

          <button
            className={`nav-item ${currentTab === "capture" ? "active" : ""}`}
            onClick={() => onSelectTab("capture")}
          >
            <div className="nav-item-left">
              <CameraIcon size={17} />
              <span>采集工作台</span>
            </div>
          </button>

          <button
            className={`nav-item ${currentTab === "settings" ? "active" : ""}`}
            onClick={() => onSelectTab("settings")}
          >
            <div className="nav-item-left">
              <SettingsIcon size={17} />
              <span>系统设置</span>
            </div>
          </button>
        </nav>
      </div>

      <div className="sidebar-footer">
        {/* Theme mode segmented toggle */}
        <div className="theme-toggle-container">
          <button
            className={`theme-toggle-btn ${theme === "light" ? "active" : ""}`}
            onClick={() => setTheme("light")}
            title="浅色模式 (默认)"
          >
            <SunIcon size={14} />
            <span>浅色</span>
          </button>
          <button
            className={`theme-toggle-btn ${theme === "auto" ? "active" : ""}`}
            onClick={() => setTheme("auto")}
            title="跟随系统"
          >
            <span>自动</span>
          </button>
          <button
            className={`theme-toggle-btn ${theme === "dark" ? "active" : ""}`}
            onClick={() => setTheme("dark")}
            title="深色模式"
          >
            <MoonIcon size={14} />
            <span>深色</span>
          </button>
        </div>

        <div className="device-status-card">
          <LaptopIcon size={16} style={{ color: "var(--text-muted)" }} />
          <div style={{ flex: 1, minWidth: 0 }}>
            <div className="device-name">{identity?.deviceName ?? "本机设备"}</div>
            <div className="device-sub" style={{ display: "flex", alignItems: "center", gap: 5 }}>
              <span className="status-dot" />
              <span>本地 E2EE 保险库就绪</span>
            </div>
          </div>
        </div>
      </div>
    </aside>
  );
}

