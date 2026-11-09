import { createContext, useContext, type ReactNode } from "react";
import { api } from "../api/client";
import type { Card } from "../api/types";
import { useDemo } from "./demo";
import { useApi, type ApiState } from "./useApi";

export type VenueState = {
  slug: string;
  card: ApiState<Card>;
  /** Venue UTC offset in minutes (falls back to Seoul until the card has loaded). */
  tz: number;
  city: string | null;
};

const VenueContext = createContext<VenueState | null>(null);

export function VenueProvider({ slug, children }: { slug: string; children: ReactNode }) {
  const { epoch } = useDemo();
  const card = useApi(() => api.card(slug), [slug, epoch]);
  const tz = card.data?.venue.tz_offset_minutes ?? 540;
  const city = card.data?.venue.city ?? null;
  return <VenueContext.Provider value={{ slug, card, tz, city }}>{children}</VenueContext.Provider>;
}

export function useVenue(): VenueState {
  const ctx = useContext(VenueContext);
  if (!ctx) throw new Error("useVenue outside VenueProvider");
  return ctx;
}

/** The venue context when inside a venue route, else null (the top bar is shown on every page). */
export function useOptionalVenue(): VenueState | null {
  return useContext(VenueContext);
}
