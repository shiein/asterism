import {
  ClipboardIcon,
  StarIcon,
  CameraIcon,
  SettingsIcon,
  LaptopIcon,
} from "./icons";
import type { DeviceIdentity } from "../types";

export type NavTab = "history" | "favorites" | "capture" | "settings";

interface SidebarProps {
  currentTab: NavTab;
  onSelectTab: (tab: NavTab) => void;
  identity?: DeviceIdentity | null;
  historyCount?: number;
  favCount?: number;
  isOnline?: boolean;
}

export function Sidebar({
  currentTab,
  onSelectTab,
  identity,
  historyCount,
  favCount,
  isOnline = true,
}: SidebarProps) {
  return (
    <aside className="sidebar">
      <div>
        <div className="brand-section">
          <div className="brand-icon">
            <ClipboardIcon size={16} />
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
            {historyCount !== undefined && historyCount > 0 && (
              <span className="nav-counter">{historyCount}</span>
            )}
          </button>

          <button
            className={`nav-item ${currentTab === "favorites" ? "active" : ""}`}
            onClick={() => onSelectTab("favorites")}
          >
            <div className="nav-item-left">
              <StarIcon size={17} filled={currentTab === "favorites"} />
              <span>收藏夹</span>
            </div>
            {favCount !== undefined && favCount > 0 && (
              <span className="nav-counter">{favCount}</span>
            )}
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
        <div className="device-status-card">
          <LaptopIcon size={16} style={{ color: "var(--text-muted)" }} />
          <div style={{ flex: 1, minWidth: 0 }}>
            <div className="device-name">{identity?.deviceName ?? "本机设备"}</div>
            <div className="device-sub" style={{ display: "flex", alignItems: "center", gap: 5 }}>
              <span className={`status-dot ${isOnline ? "" : "offline"}`} />
              <span>{isOnline ? "E2EE 保险库就绪" : "离线模式"}</span>
            </div>
          </div>
        </div>
      </div>
    </aside>
  );
}
