export interface PairingOffer {
  code: string;
  expires_at_ms: number;
  account_id: string;
}

export interface Session {
  token: string;
  account_id: string;
  device_id: string;
}

export interface HistoryItem {
  id: string;
  origin_device_id: string;
  kind: string;
  created_at_ms: number;
  logical_size: number;
  payload_size: number;
  dedup_tag: string;
  flags: number;
  encrypted_metadata: string;
  blob_id: string | null;
}

export interface Device {
  id: string;
  name: string;
  platform: string;
  last_seen_at_ms: number;
  revoked: boolean;
}

export class HubApi {
  constructor(
    public base: string,
    public token?: string,
  ) {}

  private headers(): HeadersInit {
    const h: Record<string, string> = { "content-type": "application/json" };
    if (this.token) h.authorization = `Bearer ${this.token}`;
    return h;
  }

  async pairingStart(): Promise<PairingOffer> {
    const res = await fetch(`${this.base}/api/v1/pairing/start`, { method: "POST" });
    if (!res.ok) throw new Error(`pairing start ${res.status}`);
    return res.json();
  }

  async pairingFinish(body: {
    code: string;
    device_id: string;
    device_name: string;
    platform: string;
    identity_public_key: number[];
  }): Promise<Session> {
    const res = await fetch(`${this.base}/api/v1/pairing/finish`, {
      method: "POST",
      headers: this.headers(),
      body: JSON.stringify(body),
    });
    if (!res.ok) throw new Error(`pairing finish ${res.status}`);
    return res.json();
  }

  async history(limit = 80, cursor?: string): Promise<HistoryItem[]> {
    const query = new URLSearchParams({ limit: String(limit) });
    if (cursor) query.set("cursor", cursor);
    const res = await fetch(`${this.base}/api/v1/history?${query}`, { headers: this.headers() });
    if (!res.ok) throw new Error(`history ${res.status}`);
    return res.json();
  }

  async allHistory(pageSize = 200): Promise<HistoryItem[]> {
    const items: HistoryItem[] = [];
    let cursor: string | undefined;
    for (;;) {
      const page = await this.history(pageSize, cursor);
      items.push(...page);
      if (page.length < pageSize) return items;
      const last = page.at(-1);
      if (!last) return items;
      const next = `${last.created_at_ms}:${last.id}`;
      if (next === cursor) throw new Error("history cursor did not advance");
      cursor = next;
    }
  }

  async devices(): Promise<Device[]> {
    const res = await fetch(`${this.base}/api/v1/devices`, { headers: this.headers() });
    if (!res.ok) throw new Error(`devices ${res.status}`);
    return res.json();
  }

  async deleteHistory(id: string): Promise<void> {
    const res = await fetch(`${this.base}/api/v1/history/${id}`, { method: "DELETE", headers: this.headers() });
    if (!res.ok) throw new Error(`delete ${res.status}`);
  }
}
