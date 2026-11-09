/** RFC 4122 v4 UUID (the server requires Idempotency-Key to be a UUID). */
export function uuid(): string {
  const c = globalThis.crypto;
  if (c && typeof c.randomUUID === "function") return c.randomUUID();
  const b = new Uint8Array(16);
  c.getRandomValues(b);
  b[6] = (b[6] & 0x0f) | 0x40;
  b[8] = (b[8] & 0x3f) | 0x80;
  const h = Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

// One Idempotency-Key per booking per page load: module state lives exactly as long as the page.
const depositKeys = new Map<string, string>();

export function depositKey(bookingId: string): string {
  let k = depositKeys.get(bookingId);
  if (!k) {
    k = uuid();
    depositKeys.set(bookingId, k);
  }
  return k;
}
