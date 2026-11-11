import { Fragment, useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import { api, ApiError, errorMessage } from "../../api/client";
import type { Booking, BookingSummary, ChatTurn, ToolCall } from "../../api/types";
import { IconBlock, IconCheck, IconSend, IconSpark, IconX } from "../../components/icons";
import { Spinner } from "../../components/ui";
import { localId, useChatStore, type UiMessage } from "../../lib/chatStore";
import { SlotCard } from "./SlotCard";

export const SUGGESTED_PROMPTS = [
  "A Friday night in December for about 300 people — synthwave",
  "Cheapest Saturday this month for ~100 guests",
  "Is the Rooftop free next weekend for 150 people?",
];

const AGENT_TOOLS = [
  "check_availability",
  "find_artists",
  "propose_slots",
  "request_booking",
  "get_booking_status",
  "create_deposit_order",
];

type Props = {
  slug: string;
  tz: number;
  city: string | null;
  venueName: string;
  /** after every chat turn (refetch the schedule, highlight proposals) */
  onTurn: (turn: ChatTurn) => void;
  /** after "Request this slot" */
  onBooking: (b: Booking) => void;
};

/** Plain-text reply with line breaks and "- " bullets. */
function ReplyText({ text }: { text: string }) {
  const lines = text.split(/\r?\n/);
  const blocks: { type: "p" | "ul"; lines: string[] }[] = [];
  for (const line of lines) {
    const bullet = /^\s*[-•]\s+/.test(line);
    const last = blocks[blocks.length - 1];
    if (bullet) {
      const item = line.replace(/^\s*[-•]\s+/, "");
      if (last?.type === "ul") last.lines.push(item);
      else blocks.push({ type: "ul", lines: [item] });
    } else if (line.trim() === "") {
      blocks.push({ type: "p", lines: [] });
    } else if (last?.type === "p" && last.lines.length) {
      last.lines.push(line);
    } else {
      blocks.push({ type: "p", lines: [line] });
    }
  }
  return (
    <>
      {blocks
        .filter((b) => b.lines.length)
        .map((b, i) =>
          b.type === "ul" ? (
            <ul key={i}>
              {b.lines.map((l, j) => (
                <li key={j}>{l}</li>
              ))}
            </ul>
          ) : (
            <p key={i}>
              {b.lines.map((l, j) => (
                <Fragment key={j}>
                  {j > 0 && <br />}
                  {l}
                </Fragment>
              ))}
            </p>
          ),
        )}
    </>
  );
}

function ToolChips({ calls }: { calls: ToolCall[] }) {
  const [open, setOpen] = useState<number | null>(null);
  if (!calls.length) return null;
  return (
    <div className="tools">
      <div className="tools__row">
        {calls.map((c, i) => (
          <button
            type="button"
            key={i}
            className={`toolchip toolchip--${c.status}${open === i ? " is-open" : ""}`}
            onClick={() => setOpen(open === i ? null : i)}
            title={c.summary}
            aria-expanded={open === i}
          >
            <code>{c.name}</code>
            {c.status === "ok" && <IconCheck size={13} />}
            {c.status === "blocked" && (
              <>
                <IconBlock size={13} /> <span>blocked</span>
              </>
            )}
            {c.status === "error" && (
              <>
                <IconX size={13} /> <span>error</span>
              </>
            )}
          </button>
        ))}
      </div>
      {open !== null && calls[open] && <p className="tools__summary">{calls[open].summary}</p>}
    </div>
  );
}

/** A turn takes 10–40 s with the real model: say what is happening and how long it has taken. */
function Working() {
  const [secs, setSecs] = useState(0);
  useEffect(() => {
    const started = Date.now();
    const t = setInterval(() => setSecs(Math.floor((Date.now() - started) / 1000)), 1000);
    return () => clearInterval(t);
  }, []);
  const step = secs < 8 ? "Checking availability across every space…" : secs < 20 ? "Comparing dates and prices…" : "Drafting the proposal…";
  return (
    <div className="msg msg--assistant">
      <div className="msg__bubble msg__bubble--typing">
        <Spinner label={step} />
        <span className="msg__elapsed">{secs}s</span>
      </div>
      {secs >= 12 && <p className="muted small">The concierge can take up to 40 seconds for a turn.</p>}
    </div>
  );
}

export function ConciergeChat({ slug, tz, city, venueName, onTurn, onBooking }: Props) {
  const store = useChatStore();
  const conv = store.get(slug);
  const [input, setInput] = useState("");
  const [sending, setSending] = useState(false);
  const [notLive, setNotLive] = useState(false);
  /** 503: the server has no LLM key yet (calm state, not an error storm) */
  const [offline, setOffline] = useState<string | null>(null);
  const [restoring, setRestoring] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const [narrow] = useState(() => window.matchMedia("(max-width: 480px)").matches);

  // Restore a conversation after a reload (transcript only: chips and slot cards are not stored).
  useEffect(() => {
    if (conv.conversationId || conv.messages.length) return;
    const stored = store.storedConversationId(slug);
    if (!stored) return;
    let cancelled = false;
    setRestoring(true);
    api
      .conversation(stored)
      .then((c) => {
        if (cancelled) return;
        const messages: UiMessage[] = c.messages.map((m) => ({
          id: localId(),
          role: m.role,
          text: m.text,
          at: m.at,
          restored: true,
        }));
        // Open proposals come back with the transcript: show their slot cards under the last reply.
        const lastAssistant = messages.map((m) => m.role).lastIndexOf("assistant");
        if (lastAssistant >= 0 && c.proposals?.length) {
          messages[lastAssistant] = { ...messages[lastAssistant], proposals: c.proposals };
        }
        store.update(slug, (prev) => ({
          ...prev,
          conversationId: c.conversation_id,
          booking: c.booking ?? prev.booking,
          restoredFromServer: true,
          messages,
        }));
        if (c.booking) onBooking(c.booking);
      })
      .catch((e) => {
        if (cancelled) return;
        if (e instanceof ApiError && e.notFound) store.forget(slug);
      })
      .finally(() => !cancelled && setRestoring(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slug, store.storedConversationId]);

  // New reply: show it from its first line (tool chips + text), not the bottom of the slot cards.
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const last = conv.messages[conv.messages.length - 1];
    if (last && last.role === "assistant" && !sending) {
      const el = list.querySelector<HTMLElement>(`[data-msg="${last.id}"]`);
      if (el) {
        list.scrollTo({ top: Math.max(0, el.offsetTop - list.offsetTop - 8), behavior: "smooth" });
        return;
      }
    }
    list.scrollTo({ top: list.scrollHeight, behavior: "smooth" });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [conv.messages.length, sending]);

  const send = async (text: string) => {
    const message = text.trim();
    if (!message || sending) return;
    const userMsg: UiMessage = { id: localId(), role: "user", text: message, at: new Date().toISOString() };
    store.update(slug, (c) => ({ ...c, messages: [...c.messages, userMsg] }));
    setInput("");
    setSending(true);
    try {
      const turn = await api.chat({
        venue_slug: slug,
        conversation_id: conv.conversationId ?? undefined,
        message,
      });
      setNotLive(false);
      const reply: UiMessage = {
        id: localId(),
        role: "assistant",
        text: turn.reply,
        at: new Date().toISOString(),
        toolCalls: turn.tool_calls,
        proposals: turn.proposals,
        limited: turn.limited,
      };
      store.update(slug, (c) => ({
        ...c,
        conversationId: turn.conversation_id,
        booking: turn.booking ?? c.booking,
        freshProposalIds: turn.proposals.map((p) => p.id),
        messages: [...c.messages, reply],
      }));
      onTurn(turn);
      if (turn.booking) onBooking(turn.booking);
    } catch (e) {
      const notFound = e instanceof ApiError && e.notFound && !conv.conversationId;
      const unavailable = e instanceof ApiError && e.unavailable;
      setNotLive(notFound);
      setOffline(unavailable ? errorMessage(e) : null);
      const staleConversation = e instanceof ApiError && e.notFound && !!conv.conversationId;
      if (staleConversation) store.update(slug, (c) => ({ ...c, conversationId: null }));
      const why = notFound
        ? "The AI concierge is not live on this server yet."
        : staleConversation
          ? "That conversation no longer exists. Send your message again to start a new one."
          : unavailable
          ? "Not sent: the concierge is offline."
          : e instanceof ApiError && e.status === 409
            ? "The concierge is still answering your previous message. Wait for the reply, then try again."
            : errorMessage(e);
      store.update(slug, (c) => ({
        ...c,
        messages: c.messages.map((m) => (m.id === userMsg.id ? { ...m, error: why } : m)),
      }));
    } finally {
      setSending(false);
      inputRef.current?.focus();
    }
  };

  const requestSlot = async (p: BookingSummary) => {
    const b = await api.requestBooking(p.id);
    store.update(slug, (c) => ({ ...c, booking: b }));
    onBooking(b);
  };

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    void send(input);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      void send(input);
    }
  };

  const empty = conv.messages.length === 0;

  return (
    <section className="card chat" aria-label="AI booking concierge">
      <header className="card__head chat__head">
        <div>
          <h2 className="card__title">
            <span className="chat__badge" aria-hidden>
              <IconSpark size={15} />
            </span>
            AI booking concierge
          </h2>
          <p className="card__subtitle">Ask for a date. The agent checks every space at {venueName} and proposes open slots.</p>
        </div>
        {!empty && (!conv.booking || conv.booking.status === "cancelled") && (
          <button
            type="button"
            className="btn btn--ghost btn--sm"
            onClick={() => store.forget(slug)}
            disabled={sending}
            title="Start a new conversation"
          >
            New chat
          </button>
        )}
      </header>

      <div className="chat__list" ref={listRef} aria-live="polite">
        {empty && !restoring && (
          <div className="chat__intro">
            <p className="chat__hello">Hi! I can find an open date, quote it and send the inquiry to the venue.</p>
            <p className="muted small">Try one of these:</p>
            <div className="prompts">
              {SUGGESTED_PROMPTS.map((p) => (
                <button type="button" key={p} className="prompt" onClick={() => void send(p)} disabled={sending}>
                  {p}
                </button>
              ))}
            </div>
            <div className="agent-tools" aria-label="The agent's tools">
              <span className="agent-tools__label">Agent tools</span>
              {AGENT_TOOLS.map((t) => (
                <code key={t} className={`agent-tools__tool${t === "create_deposit_order" ? " is-locked" : ""}`}>
                  {t}
                </code>
              ))}
              <span className="agent-tools__note">Payment stays locked until the venue approves.</span>
            </div>
          </div>
        )}
        {restoring && <Spinner label="Restoring your conversation…" />}
        {conv.restoredFromServer && (
          <p className="chat__restored muted small">Conversation restored. Tool calls and slot cards are shown for new turns.</p>
        )}
        {conv.messages.map((m) =>
          m.role === "user" ? (
            <div className="msg msg--user" key={m.id} data-msg={m.id}>
              <div className="msg__bubble">{m.text}</div>
              {m.error && (
                <div className="msg__error" role="alert">
                  <span>{m.error}</span>
                  {!notLive && !offline && (
                    <button type="button" className="linkbtn" onClick={() => void send(m.text)} disabled={sending}>
                      Retry
                    </button>
                  )}
                </div>
              )}
            </div>
          ) : (
            <div className={`msg msg--assistant${m.limited ? " msg--limited" : ""}`} key={m.id} data-msg={m.id}>
              {m.limited && (
                <div className="msg__limited">
                  <strong>Daily AI limit reached.</strong> The concierge is paused to keep the demo safe.
                </div>
              )}
              {m.toolCalls && <ToolChips calls={m.toolCalls} />}
              <div className="msg__bubble">
                <ReplyText text={m.text} />
              </div>
              {m.proposals && m.proposals.length > 0 && (
                <div className="slots">
                  {m.proposals.map((p) => (
                    <SlotCard key={p.id} p={p} tz={tz} city={city} booking={conv.booking} onRequest={requestSlot} />
                  ))}
                </div>
              )}
            </div>
          ),
        )}
        {sending && <Working />}
      </div>

      {notLive && (
        <p className="chat__notlive">
          The concierge endpoint <code>POST /api/agent/chat</code> is still being built on this server.
        </p>
      )}
      {offline && !notLive && (
        <p className="chat__notlive" role="status">
          {offline} You can still browse availability below.
        </p>
      )}

      <form className="composer" onSubmit={onSubmit}>
        <textarea
          ref={inputRef}
          className="composer__input"
          rows={1}
          value={input}
          placeholder={narrow ? "Date, crowd size, genre…" : "Describe your show: date, crowd size, genre…"}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={onKeyDown}
          aria-label="Message the booking concierge"
          disabled={sending}
        />
        <button type="submit" className="btn btn--primary composer__send" disabled={sending || !input.trim()} aria-label="Send">
          <IconSend size={18} />
        </button>
      </form>
    </section>
  );
}
