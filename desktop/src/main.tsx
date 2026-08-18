import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { RecordingToolbar } from "./capture/RecordingToolbar";
import { PinWindowView } from "./capture/PinWindowView";
import "./styles.css";

const queryClient = new QueryClient();
const searchParams = new URLSearchParams(window.location.search);
const isRecordingToolbar = searchParams.get("captureToolbar") === "1";
const isPinWindow = searchParams.get("pinWindow") === "1";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {isRecordingToolbar ? (
      <RecordingToolbar />
    ) : isPinWindow ? (
      <PinWindowView />
    ) : (
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>
    )}
  </StrictMode>,
);
