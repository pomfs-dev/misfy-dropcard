// Types for the HTTP API. Source of truth: docs/API.md (the server/frontend contract).
// Money is an integer number of cents (USD). Times are RFC 3339 strings in UTC.

export type Session = { session_id: string; now: string; clock_offset_seconds: number };

export type AppConfig = {
  paypal_client_id: string | null;
  paypal_env: "sandbox";
  currency: "USD";
  llm_enabled: boolean;
  webhook_verification: boolean;
};

export type Venue = {
  id: string;
  slug: string;
  name: string;
  city: string;
  country: string;
  tagline: string;
  description: string;
  tags: string[];
  capacity_band: string;
  tz_offset_minutes: number;
};

export type Space = {
  id: string;
  name: string;
  capacity_min: number;
  capacity_max: number;
  day_rate_cents: number;
  sort_order: number;
};

export type UpcomingShow = {
  id: string;
  title: string;
  space: string;
  starts_at: string;
  ticket_price_cents: number | null;
};

export type Card = { venue: Venue; spaces: Space[]; upcoming: UpcomingShow[] };

export type BookingStatus = "ai_proposed" | "hold" | "approved" | "paid" | "completed" | "cancelled";

export type ScheduleEvent = {
  id: string;
  space_id: string;
  title: string;
  status: BookingStatus;
  starts_at: string;
  ends_at: string;
  /** created in this visitor's session */
  mine: boolean;
  /** shared fictional sample data */
  sample: boolean;
};

export type Schedule = {
  now: string;
  from: string;
  to: string;
  tz_offset_minutes: number;
  spaces: Space[];
  events: ScheduleEvent[];
};

// ---- AI booking concierge

export type ChatMessage = { role: "user" | "assistant"; text: string; at: string };
export type ToolCall = { name: string; status: "ok" | "blocked" | "error"; summary: string };

export type ChatTurn = {
  conversation_id: string;
  reply: string;
  tool_calls: ToolCall[];
  proposals: BookingSummary[];
  booking: Booking | null;
  limited: boolean;
};

export type Conversation = {
  conversation_id: string;
  messages: ChatMessage[];
  /** open proposals of this conversation */
  proposals: BookingSummary[];
  booking: Booking | null;
};

// ---- Bookings

export type BookingSummary = {
  id: string;
  title: string;
  status: BookingStatus;
  space_id: string;
  space_name: string;
  starts_at: string;
  ends_at: string;
  quote_cents: number;
  deposit_cents: number;
  headcount: number | null;
  genre: string | null;
  created_by: "sample" | "agent" | "owner" | "promoter";
};

export type ConfirmedBy = "webhook" | "reconciliation" | null;

export type DepositOrder = {
  paypal_order_id: string | null;
  status: string;
  amount_cents: number;
  captured_at: string | null;
  confirmed_by: ConfirmedBy;
};

export type Booking = BookingSummary & {
  venue_slug: string;
  artist: { slug: string; name: string } | null;
  ticket_price_cents: number | null;
  inquiry_text: string | null;
  approved_at: string | null;
  conversation_id: string | null;
  deposit_order: DepositOrder | null;
  settlement: Settlement | null;
  timeline: { at: string; label: string }[];
};

export type BookingPatch = { space_id?: string; starts_at?: string; ends_at?: string };

// ---- Payments (PayPal Orders v2)

/** Response of both deposit-order and ticket-order. */
export type OrderResponse = { paypal_order_id: string; amount_cents: number; currency: "USD" };
export type CaptureResponse = {
  status: "pending_webhook" | "captured" | "failed";
  message: string | null;
  booking: Booking;
};

// ---- Venue owner console

export type SettlementStatus = "planned" | "submitted" | "succeeded" | "partial" | "failed";
export type PayoutItemStatus = "planned" | "pending" | "succeeded" | "failed" | "unclaimed" | "returned";

export type SettlementItem = {
  role: "venue" | "artist" | "promoter";
  recipient_email: string;
  amount_cents: number;
  status: PayoutItemStatus;
  paypal_item_id: string | null;
  attempts: number;
  error: string | null;
};

export type Settlement = {
  id: string;
  booking_id: string;
  booking_title: string;
  sender_batch_id: string;
  paypal_batch_id: string | null;
  status: SettlementStatus;
  scheduled_for: string;
  net_cents: number;
  platform_fee_cents: number;
  sample: boolean;
  submitted_at: string | null;
  completed_at: string | null;
  last_error: string | null;
  items: SettlementItem[];
};

export type PaymentOrder = {
  id: string;
  booking_id: string;
  booking_title: string;
  kind: "deposit" | "ticket";
  source: "paypal_sandbox" | "sample";
  paypal_order_id: string | null;
  request_id: string;
  quantity: number;
  amount_cents: number;
  paypal_fee_cents: number | null;
  status: string;
  payer_country: string | null;
  confirmed_by: ConfirmedBy;
  created_at: string;
  captured_at: string | null;
};

export type WebhookEvent = {
  id: number;
  event_type: string;
  resource_id: string | null;
  signature_valid: boolean;
  received_at: string;
  outcome: string | null;
  error: string | null;
};

export type Payments = { orders: PaymentOrder[]; webhooks: WebhookEvent[] };
export type Payouts = { batches: Settlement[] };

export type AgentRunKind = "user" | "assistant" | "tool_call" | "tool_result" | "blocked" | "error";

export type AgentRun = {
  conversation_id: string;
  step: number;
  kind: AgentRunKind;
  tool_name: string | null;
  input: unknown;
  output: unknown;
  latency_ms: number | null;
  input_tokens: number | null;
  output_tokens: number | null;
  created_at: string;
};

export type DashboardShow = {
  booking_id: string;
  date: string;
  title: string;
  space: string;
  status: BookingStatus;
  sample: boolean;
  tickets_sold: number;
  gross_cents: number;
  paypal_fees_cents: number;
  refunds_cents: number;
  platform_fee_cents: number;
  venue_cents: number;
  artist_cents: number;
  promoter_cents: number;
  payout_status: string | null;
};

export type Dashboard = {
  shows: DashboardShow[];
  by_country: { country: string; tickets: number; gross_cents: number }[];
  totals: {
    gross_cents: number;
    paypal_fees_cents: number;
    platform_fee_cents: number;
    paid_out_cents: number;
    /** 0..1, share of ticket revenue from buyers outside the venue's country */
    international_share: number;
  };
};
