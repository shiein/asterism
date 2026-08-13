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
