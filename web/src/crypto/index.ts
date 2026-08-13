const VAULT_KEY = "asterism.vault.unlocked";

export function persistUnlock(recoveryHex: string) {
  sessionStorage.setItem(VAULT_KEY, recoveryHex.trim().toLowerCase());
}

export function loadUnlock(): string | null {
  return sessionStorage.getItem(VAULT_KEY);
}

export function wipeUnlock() {
  sessionStorage.removeItem(VAULT_KEY);
}

export function vaultReady(): boolean {
  const hex = loadUnlock();
  return !!hex && /^[0-9a-f]{64}$/.test(hex);
}

export function hexToBytes(hex: string): Uint8Array {
  const clean = hex.trim();
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  return out;
}

export function bytesToHex(bytes: Uint8Array): string {
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function fromJsonBytes(value: number[] | Uint8Array): Uint8Array {
  return value instanceof Uint8Array ? value : Uint8Array.from(value);
}

/** 与 crates/crypto 相同：HKDF-SHA256 + XChaCha20-Poly1305。WASM 构建后可替换为本函数的调用点。 */
export async function decryptPackage(
  recoveryHex: string,
  packageHex: string,
): Promise<{ meta: string; body: Uint8Array | null; chunkCount: number }> {
  const pkg = JSON.parse(new TextDecoder().decode(hexToBytes(packageHex))) as {
    meta: EncryptedPayload;
    body: EncryptedPayload | null;
    chunk_count?: number;
  };
  const avk = hexToBytes(recoveryHex);
  const meta = new TextDecoder().decode(await decryptPayload(avk, pkg.meta));
  const body = pkg.body ? await decryptPayload(avk, pkg.body) : null;
  return { meta, body, chunkCount: pkg.chunk_count ?? 0 };
}

export function textFromDecrypt(metaJson: string, body: Uint8Array | null): string {
  if (body && body.length) {
    try {
      return new TextDecoder().decode(body);
    } catch {
      /* fall through */
    }
  }
  try {
    const meta = JSON.parse(metaJson) as { text_preview?: string };
    return meta.text_preview ?? "";
  } catch {
    return "";
  }
}

const BLOB_CHUNK_HEADER = 4 + 24 + 48 + 32 + 4 + 24 + 4;

export async function decryptBlobChunks(recoveryHex: string, encoded: Uint8Array[]): Promise<Uint8Array> {
  if (encoded.length === 0) throw new Error("encrypted blob has no chunks");
  const avk = hexToBytes(recoveryHex);
  const wrapKey = await hkdfSha256(avk, "asterism/item-key-wrap/v1");
  const parts: Uint8Array[] = [];
  let expectedBlob: string | null = null;
  for (let i = 0; i < encoded.length; i++) {
    const chunk = decodeBlobChunk(encoded[i]);
    if (chunk.chunkIndex !== i) throw new Error("blob chunk index is not contiguous");
    const blobHex = bytesToHex(chunk.blobId);
    if (expectedBlob && expectedBlob !== blobHex) throw new Error("blob id changed between chunks");
    expectedBlob = blobHex;
    const itemKey = await xchachaDecrypt(wrapKey, chunk.wrapNonce, chunk.wrapCipher);
    const aad = new Uint8Array(36);
    aad.set(chunk.blobId.subarray(0, 32), 0);
    new DataView(aad.buffer).setUint32(32, chunk.chunkIndex, true);
    parts.push(await xchachaDecrypt(itemKey, chunk.nonce, chunk.ciphertext, aad));
  }
  const total = parts.reduce((n, p) => n + p.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function decodeBlobChunk(bytes: Uint8Array): {
  wrapNonce: Uint8Array;
  wrapCipher: Uint8Array;
  blobId: Uint8Array;
  chunkIndex: number;
  nonce: Uint8Array;
  ciphertext: Uint8Array;
} {
  if (bytes.length < BLOB_CHUNK_HEADER) throw new Error("invalid encrypted blob chunk");
  const magic = String.fromCharCode(bytes[0], bytes[1], bytes[2], bytes[3]);
  if (magic !== "ASB1") throw new Error("invalid encrypted blob chunk");
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const chunkIndex = view.getUint32(108, true);
  const ciphertextLen = view.getUint32(136, true);
  if (bytes.length !== BLOB_CHUNK_HEADER + ciphertextLen) throw new Error("encrypted blob chunk length mismatch");
  return {
    wrapNonce: bytes.subarray(4, 28),
    wrapCipher: bytes.subarray(28, 76),
    blobId: bytes.subarray(76, 108),
    chunkIndex,
    nonce: bytes.subarray(112, 136),
    ciphertext: bytes.subarray(BLOB_CHUNK_HEADER),
  };
}

interface EncryptedPayload {
  wrapped_key: { nonce: number[]; ciphertext: number[] };
  chunk: { blob_id: number[]; chunk_index: number; nonce: number[]; ciphertext: number[] };
}

async function decryptPayload(avk: Uint8Array, payload: EncryptedPayload): Promise<Uint8Array> {
  const wrapKey = await hkdfSha256(avk, "asterism/item-key-wrap/v1");
  const itemKey = await xchachaDecrypt(wrapKey, fromJsonBytes(payload.wrapped_key.nonce), fromJsonBytes(payload.wrapped_key.ciphertext));
  const aad = new Uint8Array(36);
  aad.set(fromJsonBytes(payload.chunk.blob_id).slice(0, 32), 0);
  const view = new DataView(aad.buffer);
  view.setUint32(32, payload.chunk.chunk_index, true);
  return xchachaDecrypt(itemKey, fromJsonBytes(payload.chunk.nonce), fromJsonBytes(payload.chunk.ciphertext), aad);
}

async function hkdfSha256(ikm: Uint8Array, info: string): Promise<Uint8Array> {
  const key = await crypto.subtle.importKey("raw", ikm.buffer as ArrayBuffer, "HKDF", false, ["deriveBits"]);
  const bits = await crypto.subtle.deriveBits(
    { name: "HKDF", hash: "SHA-256", salt: new Uint8Array(), info: new TextEncoder().encode(info) },
    key,
    256,
  );
  return new Uint8Array(bits);
}

async function xchachaDecrypt(
  key: Uint8Array,
  nonce: Uint8Array,
  ciphertext: Uint8Array,
  aad: Uint8Array = new Uint8Array(),
): Promise<Uint8Array> {
  if (nonce.length !== 24) throw new Error("nonce");
  // WebCrypto 无 XChaCha20。使用 HChaCha20 派生子密钥后退化为 ChaCha20-Poly1305。
  const subkey = hchacha20(key, nonce.subarray(0, 16));
  const chachaNonce = new Uint8Array(12);
  chachaNonce.set(nonce.subarray(16, 24), 4);
  const cryptoKey = await crypto.subtle.importKey("raw", subkey.buffer as ArrayBuffer, "HKDF", false, ["deriveBits"]);
  void cryptoKey;
  return chacha20poly1305Decrypt(subkey, chachaNonce, ciphertext, aad);
}

function hchacha20(key: Uint8Array, nonce16: Uint8Array): Uint8Array {
  const sigma = new TextEncoder().encode("expand 32-byte k");
  let state = new Uint32Array(16);
  const dv = (u: Uint8Array, o: number) => new DataView(u.buffer, u.byteOffset, u.byteLength).getUint32(o, true);
  for (let i = 0; i < 4; i++) state[i] = dv(sigma, i * 4);
  for (let i = 0; i < 8; i++) state[4 + i] = dv(key, i * 4);
  for (let i = 0; i < 4; i++) state[12 + i] = dv(nonce16, i * 4);
  for (let i = 0; i < 10; i++) {
    state.set(doubleRound(state));
  }
  const out = new Uint8Array(32);
  const view = new DataView(out.buffer);
  view.setUint32(0, state[0], true);
  view.setUint32(4, state[1], true);
  view.setUint32(8, state[2], true);
  view.setUint32(12, state[3], true);
  view.setUint32(16, state[12], true);
  view.setUint32(20, state[13], true);
  view.setUint32(24, state[14], true);
  view.setUint32(28, state[15], true);
  return out;
}

function doubleRound(s: Uint32Array): Uint32Array {
  const x = Uint32Array.from(s);
  const qr = (a: number, b: number, c: number, d: number) => {
    x[a] = (x[a] + x[b]) >>> 0; x[d] = rotl(x[d] ^ x[a], 16);
    x[c] = (x[c] + x[d]) >>> 0; x[b] = rotl(x[b] ^ x[c], 12);
    x[a] = (x[a] + x[b]) >>> 0; x[d] = rotl(x[d] ^ x[a], 8);
    x[c] = (x[c] + x[d]) >>> 0; x[b] = rotl(x[b] ^ x[c], 7);
  };
  qr(0, 4, 8, 12); qr(1, 5, 9, 13); qr(2, 6, 10, 14); qr(3, 7, 11, 15);
  qr(0, 5, 10, 15); qr(1, 6, 11, 12); qr(2, 7, 8, 13); qr(3, 4, 9, 14);
  return x;
}

function rotl(v: number, n: number): number {
  return ((v << n) | (v >>> (32 - n))) >>> 0;
}

function chacha20poly1305Decrypt(key: Uint8Array, nonce12: Uint8Array, ciphertext: Uint8Array, aad: Uint8Array): Uint8Array {
  if (ciphertext.length < 16) throw new Error("ciphertext");
  const ct = ciphertext.subarray(0, ciphertext.length - 16);
  const tag = ciphertext.subarray(ciphertext.length - 16);
  const block0 = chacha20Block(key, nonce12, 0);
  const polyKey = block0.subarray(0, 32);
  const otk = poly1305(polyKey, padMac(aad, ct));
  if (!timingSafeEqual(otk, tag)) throw new Error("tag");
  const out = new Uint8Array(ct.length);
  let offset = 0;
  let counter = 1;
  while (offset < ct.length) {
    const block = chacha20Block(key, nonce12, counter++);
    const n = Math.min(64, ct.length - offset);
    for (let i = 0; i < n; i++) out[offset + i] = ct[offset + i] ^ block[i];
    offset += n;
  }
  return out;
}

function chacha20Block(key: Uint8Array, nonce12: Uint8Array, counter: number): Uint8Array {
  const sigma = new TextEncoder().encode("expand 32-byte k");
  const s = new Uint32Array(16);
  const dv = (u: Uint8Array, o: number) => new DataView(u.buffer, u.byteOffset, u.byteLength).getUint32(o, true);
  for (let i = 0; i < 4; i++) s[i] = dv(sigma, i * 4);
  for (let i = 0; i < 8; i++) s[4 + i] = dv(key, i * 4);
  s[12] = counter >>> 0;
  for (let i = 0; i < 3; i++) s[13 + i] = dv(nonce12, i * 4);
  const x = Uint32Array.from(s);
  const qr = (a: number, b: number, c: number, d: number) => {
    x[a] = (x[a] + x[b]) >>> 0; x[d] = rotl(x[d] ^ x[a], 16);
    x[c] = (x[c] + x[d]) >>> 0; x[b] = rotl(x[b] ^ x[c], 12);
    x[a] = (x[a] + x[b]) >>> 0; x[d] = rotl(x[d] ^ x[a], 8);
    x[c] = (x[c] + x[d]) >>> 0; x[b] = rotl(x[b] ^ x[c], 7);
  };
  for (let i = 0; i < 10; i++) {
    qr(0, 4, 8, 12); qr(1, 5, 9, 13); qr(2, 6, 10, 14); qr(3, 7, 11, 15);
    qr(0, 5, 10, 15); qr(1, 6, 11, 12); qr(2, 7, 8, 13); qr(3, 4, 9, 14);
  }
  const out = new Uint8Array(64);
  const view = new DataView(out.buffer);
  for (let i = 0; i < 16; i++) view.setUint32(i * 4, (x[i] + s[i]) >>> 0, true);
  return out;
}

function padMac(aad: Uint8Array, ct: Uint8Array): Uint8Array {
  const aadPad = (16 - (aad.length % 16)) % 16;
  const ctPad = (16 - (ct.length % 16)) % 16;
  const msg = new Uint8Array(aad.length + aadPad + ct.length + ctPad + 16);
  msg.set(aad, 0);
  msg.set(ct, aad.length + aadPad);
  const view = new DataView(msg.buffer);
  view.setUint32(msg.length - 16, aad.length, true);
  view.setUint32(msg.length - 8, ct.length, true);
  return msg;
}

function poly1305(key: Uint8Array, msg: Uint8Array): Uint8Array {
  const r = new Uint8Array(key.subarray(0, 16));
  r[3] &= 15; r[7] &= 15; r[11] &= 15; r[15] &= 15;
  r[4] &= 252; r[8] &= 252; r[12] &= 252;
  const rBig = le16(r);
  const sBig = le16(key.subarray(16, 32));
  let acc = 0n;
  const p = (1n << 130n) - 5n;
  for (let i = 0; i < msg.length; i += 16) {
    const chunk = msg.subarray(i, Math.min(i + 16, msg.length));
    const n = le16(chunk) + (1n << BigInt(8 * chunk.length));
    acc = ((acc + n) * rBig) % p;
  }
  acc = (acc + sBig) & ((1n << 128n) - 1n);
  const out = new Uint8Array(16);
  const view = new DataView(out.buffer);
  for (let i = 0; i < 4; i++) view.setUint32(i * 4, Number((acc >> BigInt(i * 32)) & 0xffffffffn), true);
  return out;
}

function le16(bytes: Uint8Array): bigint {
  let n = 0n;
  for (let i = bytes.length - 1; i >= 0; i--) n = (n << 8n) + BigInt(bytes[i]);
  return n;
}

function timingSafeEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  let x = 0;
  for (let i = 0; i < a.length; i++) x |= a[i] ^ b[i];
  return x === 0;
}
