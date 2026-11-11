import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { Booking, BookingSummary, ToolCall, VenueHit } from "../api/types";
import { useDemo } from "./demo";

export type UiMessage = {
  id: string;
  role: "user" | "assistant";
  text: string;
  at: string;
  toolCalls?: ToolCall[];
  proposals?: BookingSummary[];
  /** the AI venue scout's search results for this reply */
  venues?: VenueHit[];
  limited?: boolean;
  /** the request for this user message failed */
  error?: string;
  /** restored from the server (tool calls and proposals are not part of the transcript) */
  restored?: boolean;
};

export type Conversation = {
  conversationId: string | null;
  messages: UiMessage[];
  booking: Booking | null;
  /** the booking form's latest open slots */
  proposals: BookingSummary[];
  /** ids of the proposals made in the latest turn (highlighted on the scheduler) */
  freshProposalIds: string[];
  restoredFromServer: boolean;
};

const EMPTY: Conversation = {
  conversationId: null,
  messages: [],
  booking: null,
  proposals: [],
  freshProposalIds: [],
  restoredFromServer: false,
};

type Store = {
  get: (slug: string) => Conversation;
  update: (slug: string, fn: (c: Conversation) => Conversation) => void;
  storedConversationId: (slug: string) => string | null;
  storedBookingId: (slug: string) => string | null;
  forget: (slug: string) => void;
};

const ChatContext = createContext<Store | null>(null);

function storageKey(sessionId: string, slug: string) {
  return `dropcard.conversation.${sessionId}.${slug}`;
}

function bookingKey(sessionId: string, slug: string) {
  return `dropcard.booking.${sessionId}.${slug}`;
}

function readStorage(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStorage(key: string, value: string | null) {
  try {
    if (value === null) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, value);
  } catch {
    /* private mode or blocked storage: the conversation just won't survive a reload */
  }
}

/** Keeps each venue's conversation while the visitor switches between promoter and owner views. */
export function ChatStoreProvider({ children }: { children: ReactNode }) {
  const { session, resetEpoch } = useDemo();
  const [convs, setConvs] = useState<Record<string, Conversation>>({});
  const sessionId = session?.session_id ?? null;
  const sessionRef = useRef(sessionId);
  sessionRef.current = sessionId;

  // "Reset my demo" deletes the conversations on the server: drop them here too.
  const firstReset = useRef(resetEpoch);
  useEffect(() => {
    if (resetEpoch === firstReset.current) return;
    setConvs((prev) => {
      const sid = sessionRef.current;
      if (sid) {
        Object.keys(prev).forEach((slug) => {
          writeStorage(storageKey(sid, slug), null);
          writeStorage(bookingKey(sid, slug), null);
        });
      }
      return {};
    });
  }, [resetEpoch]);

  const get = useCallback((slug: string) => convs[slug] ?? EMPTY, [convs]);

  const update = useCallback((slug: string, fn: (c: Conversation) => Conversation) => {
    setConvs((prev) => {
      const next = fn(prev[slug] ?? EMPTY);
      const sid = sessionRef.current;
      if (sid) {
        if (next.conversationId) writeStorage(storageKey(sid, slug), next.conversationId);
        writeStorage(bookingKey(sid, slug), next.booking ? next.booking.id : null);
      }
      return { ...prev, [slug]: next };
    });
  }, []);

  const storedConversationId = useCallback(
    (slug: string) => (sessionId ? readStorage(storageKey(sessionId, slug)) : null),
    [sessionId],
  );

  const storedBookingId = useCallback(
    (slug: string) => (sessionId ? readStorage(bookingKey(sessionId, slug)) : null),
    [sessionId],
  );

  const forget = useCallback(
    (slug: string) => {
      if (sessionId) {
        writeStorage(storageKey(sessionId, slug), null);
        writeStorage(bookingKey(sessionId, slug), null);
      }
      setConvs((prev) => ({ ...prev, [slug]: EMPTY }));
    },
    [sessionId],
  );

  const value = useMemo(
    () => ({ get, update, storedConversationId, storedBookingId, forget }),
    [get, update, storedConversationId, storedBookingId, forget],
  );
  return <ChatContext.Provider value={value}>{children}</ChatContext.Provider>;
}

export function useChatStore(): Store {
  const ctx = useContext(ChatContext);
  if (!ctx) throw new Error("useChatStore outside ChatStoreProvider");
  return ctx;
}

let counter = 0;
export function localId(prefix = "m"): string {
  counter += 1;
  return `${prefix}-${Date.now().toString(36)}-${counter}`;
}
