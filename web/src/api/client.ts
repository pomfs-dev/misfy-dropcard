import type {
  AgentRun,
  AppConfig,
  ArtistItem,
  Booking,
  BookingPatch,
  CaptureResponse,
  Card,
  ChatTurn,
  Conversation,
  Dashboard,
  OrderResponse,
  Payments,
  Payouts,
  ProposalRequest,
  ProposalResult,
  Schedule,
  Session,
  Venue,
} from "./types";

/** A non-2xx answer from the server (`{"error": "..."}`) or a network failure (status 0). */
export class ApiError extends Error {
  readonly status: number;
  readonly method: string;
  readonly path: string;

  constructor(status: number, message: string, method: string, path: string) {
    // Server messages are sentence fragments ("that time overlaps another booking"): capitalize for display.
    super(message ? message.charAt(0).toUpperCase() + message.slice(1) : message);
    this.name = "ApiError";
    this.status = status;
    this.method = method;
    this.path = path;
  }

  /** 404: unknown route (endpoint not live yet) or missing record. */
  get notFound(): boolean {
    return this.status === 404;
  }

  /** 503: a dependency (PayPal, the LLM) is not configured on this server. */
  get unavailable(): boolean {
    return this.status === 503;
  }
}

export function isApiError(e: unknown): e is ApiError {
  return e instanceof ApiError;
}

export function errorMessage(e: unknown): string {
  if (e instanceof ApiError) return e.message;
  if (e instanceof Error) return e.message;
  return "Something went wrong.";
}

type Method = "GET" | "POST" | "PATCH";

async function request<T>(
  method: Method,
  path: string,
  body?: unknown,
  headers?: Record<string, string>,
): Promise<T> {
  const init: RequestInit = {
    method,
    credentials: "same-origin",
    headers: { Accept: "application/json", ...headers },
  };
  if (method !== "GET") {
    // Always send a JSON body on writes: handlers with a Json extractor reject an empty body.
    init.body = JSON.stringify(body ?? {});
    (init.headers as Record<string, string>)["Content-Type"] = "application/json";
  }

  let res: Response;
  try {
    res = await fetch(path, init);
  } catch {
    throw new ApiError(0, "Can't reach the DropCard server. Check that it is running.", method, path);
  }

  const text = await res.text();
  let data: unknown = null;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = null;
    }
  }
  if (!res.ok) {
    const msg =
      data && typeof data === "object" && "error" in data && typeof (data as { error: unknown }).error === "string"
        ? (data as { error: string }).error
        : `Request failed (${res.status})`;
    throw new ApiError(res.status, msg, method, path);
  }
  return data as T;
}

const get = <T>(path: string) => request<T>("GET", path);
const post = <T>(path: string, body?: unknown, headers?: Record<string, string>) =>
  request<T>("POST", path, body, headers);
const patch = <T>(path: string, body: unknown) => request<T>("PATCH", path, body);
const enc = encodeURIComponent;

export const api = {
  // Session and demo controls
  session: () => get<Session>("/api/session"),
  advanceClock: (hours: number) => post<Session>("/api/demo/clock/advance", { hours }),
  resetClock: () => post<Session>("/api/demo/clock/reset"),
  resetDemo: () => post<Session>("/api/demo/reset"),
  config: () => get<AppConfig>("/api/config"),

  // Venues
  venues: () => get<Venue[]>("/api/venues"),
  card: (slug: string) => get<Card>(`/api/venues/${enc(slug)}`),
  schedule: (slug: string) => get<Schedule>(`/api/venues/${enc(slug)}/schedule`),
  // Booking form (no AI): up to three open slots for the promoter's dates
  proposals: (slug: string, body: ProposalRequest) => post<ProposalResult>(`/api/venues/${enc(slug)}/proposals`, body),
  artists: () => get<ArtistItem[]>("/api/artists"),

  // AI agents: the venue scout (no venue_slug; works for `artist`) and a venue's booking concierge
  chat: (body: { venue_slug?: string; artist?: string; conversation_id?: string; message: string }) =>
    post<ChatTurn>("/api/agent/chat", body),
  conversation: (id: string) => get<Conversation>(`/api/agent/conversations/${enc(id)}`),

  // Bookings
  booking: (id: string) => get<Booking>(`/api/bookings/${enc(id)}`),
  requestBooking: (id: string, inquiryText?: string) =>
    post<Booking>(`/api/bookings/${enc(id)}/request`, inquiryText ? { inquiry_text: inquiryText } : {}),
  cancelBooking: (id: string) => post<Booking>(`/api/bookings/${enc(id)}/cancel`),

  // Payments
  createDepositOrder: (id: string, idempotencyKey: string) =>
    post<OrderResponse>(`/api/bookings/${enc(id)}/deposit-order`, {}, { "Idempotency-Key": idempotencyKey }),
  createTicketOrder: (showId: string, quantity: number, idempotencyKey: string) =>
    post<OrderResponse>(`/api/shows/${enc(showId)}/ticket-order`, { quantity }, { "Idempotency-Key": idempotencyKey }),
  captureOrder: (paypalOrderId: string) => post<CaptureResponse>(`/api/paypal/orders/${enc(paypalOrderId)}/capture`),

  // Venue owner console
  inbox: (slug: string) => get<Booking[]>(`/api/console/${enc(slug)}/inbox`),
  approve: (id: string) => post<Booking>(`/api/console/bookings/${enc(id)}/approve`),
  decline: (id: string) => post<Booking>(`/api/console/bookings/${enc(id)}/decline`),
  patchBooking: (id: string, body: BookingPatch) => patch<Booking>(`/api/console/bookings/${enc(id)}`, body),
  payments: (slug: string) => get<Payments>(`/api/console/${enc(slug)}/payments`),
  payouts: (slug: string) => get<Payouts>(`/api/console/${enc(slug)}/payouts`),
  runSettlement: (slug: string) => post<Payouts>(`/api/console/${enc(slug)}/settlement/run`),
  agentRuns: (slug: string) => get<{ runs: AgentRun[] }>(`/api/console/${enc(slug)}/agent-runs`),
  dashboard: (slug: string) => get<Dashboard>(`/api/console/${enc(slug)}/dashboard`),
};
