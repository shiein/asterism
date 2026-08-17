import { create } from "zustand";
import type { ContentKind } from "./types";

export type AppTheme = "light" | "dark" | "auto";

interface UiState {
  query: string;
  kind: ContentKind | "ALL";
  favoriteOnly: boolean;
  theme: AppTheme;
  setQuery: (query: string) => void;
  setKind: (kind: ContentKind | "ALL") => void;
  setFavoriteOnly: (favoriteOnly: boolean) => void;
  setTheme: (theme: AppTheme) => void;
}

const THEMES: readonly AppTheme[] = ["light", "dark", "auto"];

function readSavedTheme(): AppTheme {
  if (typeof localStorage === "undefined") return "light";
  const saved = localStorage.getItem("asterism_theme");
  return THEMES.includes(saved as AppTheme) ? (saved as AppTheme) : "light";
}

const savedTheme = readSavedTheme();

export const useUiStore = create<UiState>((set) => ({
  query: "",
  kind: "ALL",
  favoriteOnly: false,
  theme: savedTheme,
  setQuery: (query) => set({ query }),
  setKind: (kind) => set({ kind }),
  setFavoriteOnly: (favoriteOnly) => set({ favoriteOnly }),
  setTheme: (theme) => {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem("asterism_theme", theme);
    }
    set({ theme });
  },
}));

