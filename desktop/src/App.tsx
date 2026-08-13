import { useState } from "react";
import { HistoryPage } from "./history/HistoryPage";
import { SettingsPage } from "./settings/SettingsPage";

export function App() {
  const [tab, setTab] = useState<"history" | "settings">("history");
  return (
    <div>
      <nav className="filters" style={{ maxWidth: 860, margin: "16px auto 0", padding: "0 20px" }}>
        <button className={tab === "history" ? "chip on" : "chip"} onClick={() => setTab("history")}>
          历史
        </button>
        <button className={tab === "settings" ? "chip on" : "chip"} onClick={() => setTab("settings")}>
          设置
        </button>
      </nav>
      {tab === "history" ? <HistoryPage /> : <div className="app"><SettingsPage /></div>}
    </div>
  );
}
