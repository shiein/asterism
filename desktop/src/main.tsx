import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { RecordingToolbar } from "./capture/RecordingToolbar";
import "./styles.css";

const queryClient = new QueryClient();
const isRecordingToolbar = new URLSearchParams(window.location.search).get("captureToolbar") === "1";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {isRecordingToolbar ? (
      <RecordingToolbar />
    ) : (
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>
    )}
  </StrictMode>,
);
