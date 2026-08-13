import { create } from "zustand";
import type { ContentKind } from "./types";

interface UiState {
  query: string;
  kind: ContentKind | "ALL";
  favoriteOnly: boolean;
  setQuery: (query: string) => void;
  setKind: (kind: ContentKind | "ALL") => void;
  setFavoriteOnly: (favoriteOnly: boolean) => void;
}

export const useUiStore = create<UiState>((set) => ({
  query: "",
  kind: "ALL",
  favoriteOnly: false,
  setQuery: (query) => set({ query }),
  setKind: (kind) => set({ kind }),
  setFavoriteOnly: (favoriteOnly) => set({ favoriteOnly }),
}));
