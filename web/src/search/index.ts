/** Latin word/prefix + CJK 2-gram。候选再回原文验证。 */
export function tokens(text: string): string[] {
  const out = new Set<string>();
  const lower = text.toLowerCase();
  for (const word of lower.split(/[^\p{L}\p{N}]+/u).filter(Boolean)) {
    out.add(word);
    for (let i = 1; i <= word.length && i <= 8; i++) out.add(word.slice(0, i));
  }
  const cjk = [...text].filter((ch) => /\p{Script=Han}/u.test(ch));
  for (let i = 0; i < cjk.length; i++) {
    out.add(cjk[i]);
    if (i + 1 < cjk.length) out.add(cjk[i] + cjk[i + 1]);
    if (i + 2 < cjk.length) out.add(cjk[i] + cjk[i + 1] + cjk[i + 2]);
  }
  return [...out];
}

export class LocalIndex {
  private postings = new Map<string, Set<string>>();
  private docs = new Map<string, string>();

  add(id: string, text: string) {
    this.remove(id);
    this.docs.set(id, text);
    for (const t of tokens(text)) {
      let set = this.postings.get(t);
      if (!set) {
        set = new Set<string>();
        this.postings.set(t, set);
      }
      set.add(id);
    }
  }

  clear() {
    this.postings.clear();
    this.docs.clear();
  }

  remove(id: string) {
    const prev = this.docs.get(id);
    if (!prev) return;
    for (const t of tokens(prev)) this.postings.get(t)?.delete(id);
    this.docs.delete(id);
  }

  search(query: string): string[] {
    const ts = tokens(query);
    if (ts.length === 0) return Array.from(this.docs.keys());
    let acc: string[] | null = null;
    for (const t of ts) {
      const hits = this.postings.get(t) ?? new Set<string>();
      const next = Array.from(hits);
      acc = acc === null ? next : acc.filter((id) => hits.has(id));
    }
    const q = query.toLowerCase();
    return (acc ?? []).filter((id) => {
      const doc = this.docs.get(id) ?? "";
      return doc.toLowerCase().includes(q) || tokens(doc).some((token) => ts.includes(token));
    });
  }

  get size() {
    return this.docs.size;
  }
}
