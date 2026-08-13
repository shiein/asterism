import { invoke } from "@tauri-apps/api/core";
import type { ContentKind, DeviceIdentity, HistoryItem } from "./types";

export function listHistory(input: {
  query?: string;
  kind?: ContentKind;
  favoriteOnly?: boolean;
  limit?: number;
}): Promise<HistoryItem[]> {
  return invoke("list_history", {
    query: input.query ?? null,
    kind: input.kind ?? null,
    favoriteOnly: input.favoriteOnly ?? false,
    limit: input.limit ?? 80,
  });
}

export function setFavorite(id: string, favorite: boolean): Promise<void> {
  return invoke("set_favorite", { id, favorite });
}

export function deleteItem(id: string): Promise<void> {
  return invoke("delete_item", { id });
}

export function copyItem(id: string): Promise<void> {
  return invoke("copy_item", { id });
}

export function getIdentity(): Promise<DeviceIdentity> {
  return invoke("get_identity");
}
