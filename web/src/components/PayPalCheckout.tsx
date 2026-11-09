import { useEffect, useRef, useState } from "react";
import { loadScript, type PayPalNamespace } from "@paypal/paypal-js";
import { api, ApiError, errorMessage } from "../api/client";
import type { CaptureResponse } from "../api/types";
import { useDemo } from "../lib/demo";
import { Spinner } from "./ui";

const sdkCache = new Map<string, Promise<PayPalNamespace | null>>();

function loadPayPal(clientId: string): Promise<PayPalNamespace | null> {
  let p = sdkCache.get(clientId);
  if (!p) {
    // English UI everywhere (the browser locale would otherwise pick the button language).
    p = loadScript({ clientId, currency: "USD", intent: "capture", components: "buttons", locale: "en_US" });
    p.catch(() => sdkCache.delete(clientId));
    sdkCache.set(clientId, p);
  }
  return p;
}

const NOT_CONFIGURED = "PayPal sandbox not configured yet";

export type CheckoutProps = {
  /** Creates the order on OUR server (which calls PayPal Orders v2) and returns the PayPal order id. */
  createOrder: () => Promise<string>;
  /** Called after our server captured the order (status "pending_webhook" or "captured"). */
  onCaptured: (res: CaptureResponse, paypalOrderId: string) => void;
  disabled?: boolean;
  /** Changing this re-renders the buttons (e.g. a new purchase). */
  renderKey?: string;
};

/**
 * PayPal JS SDK buttons. The browser never decides that a payment succeeded: after capture the
 * caller waits for the server to receive the verified webhook.
 */
export function PayPalCheckout({ createOrder, onCaptured, disabled, renderKey }: CheckoutProps) {
  const { config } = useDemo();
  const clientId = config?.value?.paypal_client_id ?? null;
  const host = useRef<HTMLDivElement>(null);
  const createRef = useRef(createOrder);
  const capturedRef = useRef(onCaptured);
  createRef.current = createOrder;
  capturedRef.current = onCaptured;
  const [phase, setPhase] = useState<"loading" | "ready" | "capturing" | "failed-load">("loading");
  const [message, setMessage] = useState<string | null>(null);
  const [unavailable, setUnavailable] = useState<string | null>(null);

  useEffect(() => {
    if (!clientId || disabled || unavailable) return;
    let closed = false;
    let buttons: { close: () => Promise<void> } | null = null;
    setPhase("loading");
    loadPayPal(clientId)
      .then((paypal) => {
        if (closed || !host.current) return;
        if (!paypal?.Buttons) {
          setPhase("failed-load");
          return;
        }
        const b = paypal.Buttons({
          fundingSource: paypal.FUNDING?.PAYPAL,
          style: { layout: "vertical", color: "black", shape: "rect", label: "pay", height: 44, tagline: false },
          createOrder: async () => {
            setMessage(null);
            try {
              return await createRef.current();
            } catch (e) {
              if (e instanceof ApiError && e.status === 503) setUnavailable(e.message);
              else setMessage(errorMessage(e));
              throw e;
            }
          },
          onApprove: async (data, actions) => {
            setPhase("capturing");
            try {
              const res = await api.captureOrder(data.orderID);
              if (res.status === "failed") {
                setMessage(res.message ?? "PayPal declined the payment. Please try again.");
                setPhase("ready");
                actions.restart();
                return;
              }
              capturedRef.current(res, data.orderID);
              setPhase("ready");
            } catch (e) {
              setMessage(errorMessage(e));
              setPhase("ready");
            }
          },
          onCancel: () => setMessage("Payment window closed. Nothing was charged; you can try again."),
          onError: (err) => {
            // createOrder failures already set a message.
            setMessage((m) => m ?? (typeof err?.message === "string" ? err.message : "PayPal could not complete the payment."));
          },
        });
        buttons = b;
        if (!b.isEligible()) {
          setPhase("failed-load");
          return;
        }
        b.render(host.current)
          .then(() => !closed && setPhase("ready"))
          .catch(() => !closed && setPhase("failed-load"));
      })
      .catch(() => !closed && setPhase("failed-load"));
    return () => {
      closed = true;
      buttons?.close().catch(() => undefined);
      if (host.current) host.current.innerHTML = "";
    };
  }, [clientId, disabled, unavailable, renderKey]);

  if (config === null) {
    return (
      <div className="paypal paypal--placeholder">
        <Spinner label="Loading PayPal…" />
      </div>
    );
  }
  if (!clientId || unavailable) {
    return (
      <div className="paypal">
        <button type="button" className="paypal-disabled" disabled>
          {NOT_CONFIGURED}
        </button>
        <p className="paypal__note">
          {unavailable ?? "The server has no PayPal sandbox client id yet, so payments are switched off."}
        </p>
      </div>
    );
  }
  return (
    <div className="paypal">
      {phase === "loading" && <Spinner label="Loading PayPal…" />}
      {phase === "failed-load" && (
        <p className="paypal__note">The PayPal button could not be loaded. Check your connection and reload the page.</p>
      )}
      {phase === "capturing" && <Spinner label="Capturing the payment with PayPal…" />}
      <div ref={host} className={`paypal__buttons${disabled ? " is-disabled" : ""}`} />
      {message && (
        <p className="paypal__error" role="alert">
          {message}
        </p>
      )}
    </div>
  );
}
