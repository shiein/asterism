import { useState, useEffect } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ToastProvider } from "./components/Toast";
import { Sidebar, type NavTab } from "./components/Sidebar";
import { HistoryPage } from "./history/HistoryPage";
import { CaptureStudioPage } from "./capture/CaptureStudioPage";
import { SettingsPage } from "./settings/SettingsPage";
import { AnnotatePage } from "./capture/AnnotatePage";
import { getIdentity } from "./api";
import { useUiStore } from "./store";

export function App() {
  return (
    <ToastProvider>
      <MainApp />
    </ToastProvider>
  );
}

function MainApp() {
  const queryClient = useQueryClient();
  const [tab, setTab] = useState<NavTab>("history");
  const [annotateId, setAnnotateId] = useState<string | null>(null);
  const theme = useUiStore((s) => s.theme);

  useEffect(() => {
    function applyTheme() {
      const isDark =
        theme === "dark" ||
        (theme === "auto" && window.matchMedia("(prefers-color-scheme: dark)").matches);
      document.documentElement.setAttribute("data-theme", isDark ? "dark" : "light");
    }

    applyTheme();

    if (theme === "auto") {
      const matcher = window.matchMedia("(prefers-color-scheme: dark)");
      matcher.addEventListener("change", applyTheme);
      return () => matcher.removeEventListener("change", applyTheme);
    }
  }, [theme]);

  const identity = useQuery({
    queryKey: ["identity"],
    queryFn: getIdentity,
  });

  function closeAnnotate() {
    setAnnotateId(null);
    void queryClient.invalidateQueries({ queryKey: ["history"] });
  }

  // Keyboard navigation shortcuts: ⌘1-4
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && !e.shiftKey) {
        if (e.key === "1") {
          e.preventDefault();
          setTab("history");
        } else if (e.key === "2") {
          e.preventDefault();
          setTab("favorites");
        } else if (e.key === "3") {
          e.preventDefault();
          setTab("capture");
        } else if (e.key === "4") {
          e.preventDefault();
          setTab("settings");
        }
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);

  return (
    <div className="app-layout">
      {/* Left Modern Sidebar */}
      <Sidebar
        currentTab={tab}
        onSelectTab={setTab}
        identity={identity.data}
      />

      {/* Main Content Area */}
      {tab === "history" && <HistoryPage onAnnotate={setAnnotateId} />}
      {tab === "favorites" && <HistoryPage onAnnotate={setAnnotateId} favoriteFilter={true} />}
      {tab === "capture" && <CaptureStudioPage onAnnotate={setAnnotateId} />}
      {tab === "settings" && <SettingsPage />}

      {/* Annotation Canvas Overlay */}
      {annotateId && <AnnotatePage itemId={annotateId} onDone={closeAnnotate} />}
    </div>
  );
}
