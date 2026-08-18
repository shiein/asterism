export type ContentKind = "TEXT" | "IMAGE" | "FILES" | "SCREENSHOT" | "GIF" | "VIDEO";

export interface HistoryItem {
  id: string;
  kind: ContentKind;
  createdAtMs: number;
  preview: string | null;
  favorite: boolean;
  sourceApp: string | null;
  logicalSize: number;
  imageWidth: number | null;
  imageHeight: number | null;
  fileCount: number | null;
}

export interface DeviceIdentity {
  deviceId: string;
  accountId: string;
  deviceName: string;
}

export interface ShortcutSettings {
  toggle_window: string;
  capture_region: string;
  capture_fullscreen: string;
  record_gif: string;
  record_video: string;
}

export interface AppSettings {
  close_to_tray: boolean;
  minimize_to_tray: boolean;
  autostart: boolean;
  shortcuts: ShortcutSettings;
}
