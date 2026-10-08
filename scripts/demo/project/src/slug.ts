// No look-alikes (l, 1, o, 0): slugs get read aloud and typed by hand.
const ALPHABET = "abcdefghijkmnpqrstuvwxyz23456789";

/** A random slug of `length` characters. */
export function slug(length = 4): string {
  const bytes = crypto.getRandomValues(new Uint8Array(length));
  return Array.from(bytes, (b) => ALPHABET[b % ALPHABET.length]).join("");
}

export function isSlug(value: string): boolean {
  return /^[a-km-np-z2-9]{4,8}$/.test(value);
}
