import { useState, useEffect } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ToastProvider } from "./components/Toast";
import { Sidebar, type NavTab } from "./components/Sidebar";
import { HistoryPage } from "./history/HistoryPage";
import { CaptureStudioPage } from "./capture/CaptureStudioPage";
import { SettingsPage } from "./settings/SettingsPage";
import { AnnotatePage } from "./capture/AnnotatePage";
import { getIdentity, listHistory } from "./api";

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

  const identity = useQuery({
    queryKey: ["identity"],
    queryFn: getIdentity,
  });

  const historyPreview = useQuery({
    queryKey: ["history-counts"],
    queryFn: () => listHistory({ limit: 100 }),
    staleTime: 10_000,
  });

  const historyCount = historyPreview.data?.length;
  const favCount = historyPreview.data?.filter((item) => item.favorite).length;

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
        historyCount={historyCount}
        favCount={favCount}
        isOnline={true}
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
