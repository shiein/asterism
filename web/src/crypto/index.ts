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

/** Web 完整解密走同一 Rust 实现的 WASM。此处只保留解锁状态与索引材料。 */
export function vaultReady(): boolean {
  const hex = loadUnlock();
  return !!hex && /^[0-9a-f]{64}$/.test(hex);
}
