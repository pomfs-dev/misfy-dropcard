import { Link } from "react-router-dom";
import { api } from "../../api/client";
import type { AgentRun, AgentRunKind } from "../../api/types";
import { Chip, EmptyState, Loadable } from "../../components/ui";
import { useDemo } from "../../lib/demo";
import { fmtStamp, shortId } from "../../lib/format";
import { useApi, usePoll } from "../../lib/useApi";
import { useVenue } from "../../lib/venue";

const KIND: Record<AgentRunKind, { label: string; tone: "neutral" | "blue" | "green" | "red" | "yellow" }> = {
  user: { label: "User", tone: "neutral" },
  assistant: { label: "Assistant", tone: "neutral" },
  tool_call: { label: "Tool call", tone: "blue" },
  tool_result: { label: "Tool result", tone: "green" },
  blocked: { label: "Blocked", tone: "red" },
  error: { label: "Error", tone: "red" },
};

function Json({ value }: { value: unknown }) {
  if (value === null || value === undefined) return <span className="muted small">—</span>;
  let text: string;
  try {
    text = typeof value === "string" ? value : JSON.stringify(value, null, 2);
  } catch {
    text = String(value);
  }
  return <pre className="json">{text}</pre>;
}

function preview(v: unknown): string {
  if (v === null || v === undefined) return "";
  if (typeof v === "string") return v;
  if (typeof v === "object" && v !== null && typeof (v as { text?: unknown }).text === "string") {
    return (v as { text: string }).text;
  }
  try {
    const s = JSON.stringify(v);
    return s.length > 110 ? `${s.slice(0, 110)}…` : s;
  } catch {
    return "";
  }
}

function Conversation({ runs, tz }: { runs: AgentRun[]; tz: number }) {
  const first = runs[0];
  const last = runs[runs.length - 1];
  const tools = runs.filter((r) => r.kind === "tool_call").length;
  const blocked = runs.filter((r) => r.kind === "blocked").length;
  const tokensIn = runs.reduce((n, r) => n + (r.input_tokens ?? 0), 0);
  const tokensOut = runs.reduce((n, r) => n + (r.output_tokens ?? 0), 0);
  return (
    <section className="card convo">
      <header className="card__head">
        <div>
          <h2 className="card__title">
            Conversation <code>{shortId(first.conversation_id)}</code>
          </h2>
          <p className="card__subtitle">
            {fmtStamp(first.created_at, tz)} – {fmtStamp(last.created_at, tz)} · {runs.length} steps · {tools} tool
            call{tools === 1 ? "" : "s"}
            {blocked ? ` · ${blocked} blocked` : ""}
            {tokensIn || tokensOut ? ` · ${tokensIn.toLocaleString()} in / ${tokensOut.toLocaleString()} out tokens` : ""}
          </p>
        </div>
      </header>
      <ol className="runs">
        {runs.map((r) => {
          const k = KIND[r.kind] ?? { label: r.kind, tone: "neutral" as const };
          return (
            <li className={`run run--${r.kind}`} key={`${r.conversation_id}-${r.step}`}>
              <details>
                <summary className="run__row">
                  <span className="run__step">{r.step}</span>
                  <Chip tone={k.tone}>{k.label}</Chip>
                  {r.tool_name && <code className="run__tool">{r.tool_name}</code>}
                  <span className="run__preview">
                    {preview(r.kind === "tool_result" || r.kind === "assistant" ? r.output ?? r.input : r.input ?? r.output)}
                  </span>
                  <span className="run__meta">
                    {r.latency_ms !== null && <span>{r.latency_ms.toLocaleString()} ms</span>}
                    {(r.input_tokens !== null || r.output_tokens !== null) && (
                      <span>
                        {r.input_tokens ?? 0}/{r.output_tokens ?? 0} tok
                      </span>
                    )}
                    <time>{fmtStamp(r.created_at, tz)}</time>
                  </span>
                </summary>
                <div className="run__body">
                  <div>
                    <h4>Input</h4>
                    <Json value={r.input} />
                  </div>
                  <div>
                    <h4>Output</h4>
                    <Json value={r.output} />
                  </div>
                </div>
              </details>
            </li>
          );
        })}
      </ol>
    </section>
  );
}

export function AgentTab() {
  const { slug, tz } = useVenue();
  const { epoch } = useDemo();
  const runs = useApi(() => api.agentRuns(slug), [slug, epoch]);
  usePoll(
    () => api.agentRuns(slug),
    5000,
    runs.data !== null,
    (r) => {
      runs.setData(r);
      return false;
    },
  );

  return (
    <Loadable state={runs} what="Agent activity" endpoint={`GET /api/console/${slug}/agent-runs`}>
      {(data) => {
        if (data.runs.length === 0) {
          return (
            <EmptyState title="No agent activity yet">
              Every message, tool call and blocked action of the AI concierge is recorded here. Start a chat in the{" "}
              <Link to={`/v/${slug}`}>Promoter view</Link>.
            </EmptyState>
          );
        }
        const groups = new Map<string, AgentRun[]>();
        for (const r of data.runs) {
          const g = groups.get(r.conversation_id) ?? [];
          g.push(r);
          groups.set(r.conversation_id, g);
        }
        const convos = [...groups.values()]
          .map((g) => g.sort((a, b) => a.step - b.step))
          .sort((a, b) => Date.parse(b[b.length - 1].created_at) - Date.parse(a[a.length - 1].created_at));
        return (
          <div className="stack">
            <p className="muted small">
              The agent may only create a PayPal deposit order after the venue approved; earlier attempts are recorded as
              blocked.
            </p>
            {convos.map((g) => (
              <Conversation key={g[0].conversation_id} runs={g} tz={tz} />
            ))}
          </div>
        );
      }}
    </Loadable>
  );
}
