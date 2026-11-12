/**
 * Demo photos: CC0 stock images in /demo (see web/public/demo/CREDITS.md). They never show the real venue or artist:
 * directory venues are pseudonymized and the sample artists are made up, so each one just gets a stock photo for looks.
 */
const VENUE_PHOTOS = 12;
const ARTISTS_WITH_PHOTOS = new Set(["hollow-arcade", "brass-comet", "moonlit-static", "paper-lanterns-club"]);

function hash(s: string): number {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

/** A stable stock photo for a venue (the same venue always gets the same one). */
export function venuePhoto(slug: string): string {
  const n = (hash(slug) % VENUE_PHOTOS) + 1;
  return `/demo/venue-${String(n).padStart(2, "0")}.jpg`;
}

/** The stock photo of a sample artist, or null for artists without one. */
export function artistPhoto(slug: string | null | undefined): string | null {
  return slug && ARTISTS_WITH_PHOTOS.has(slug) ? `/demo/artist-${slug}.jpg` : null;
}

export const PHOTO_NOTE = "Photos are CC0 stock images, not the real venues or artists.";
