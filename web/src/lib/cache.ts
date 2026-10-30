/** Keeps the most recently used entries; search queries must not grow a session forever. */
export class BoundedCache<K, V> {
  private readonly entries = new Map<K, V>();

  constructor(private readonly limit: number) {}

  get(key: K): V | undefined {
    const value = this.entries.get(key);
    if (this.entries.has(key)) {
      this.entries.delete(key);
      this.entries.set(key, value as V);
    }
    return value;
  }

  set(key: K, value: V): void {
    this.entries.delete(key);
    this.entries.set(key, value);
    if (this.entries.size > this.limit) {
      this.entries.delete(this.entries.keys().next().value as K);
    }
  }
}
