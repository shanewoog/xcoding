import { invoke } from "@tauri-apps/api/core";
import "./RequestLogs.css";
import { useCallback, useEffect, useRef, useState } from "react";
import { t, type Locale, type MessageKey } from "./i18n";

type LogStatus = "pending" | "success" | "error" | "interrupted";
interface LogSummary {
  id: string;
  created_at: string;
  model: string;
  endpoint: string;
  status: LogStatus;
  http_status: number | null;
  duration_ms: number;
}
interface LogDetail extends LogSummary {
  request_body: string;
  request_headers?: { name: string; value: string }[] | null;
  response_body: string;
  response_content_type: string | null;
  error: string | null;
  truncated: boolean;
}
interface LogPage { items: LogSummary[]; has_more: boolean }
const emptyFilters = { from: "", to: "", model: "", endpoint: "", status: "" };
const statusKeys: Record<LogStatus, MessageKey> = {
  pending: "requestLogs.pending", success: "requestLogs.success",
  error: "requestLogs.error", interrupted: "requestLogs.interrupted",
};

export function RequestLogs({ locale, enabled, onEnabledChange, disabled }: {
  locale: Locale;
  enabled: boolean;
  onEnabledChange: (enabled: boolean) => void;
  disabled: boolean;
}) {
  const [filters, setFilters] = useState(emptyFilters);
  const [applied, setApplied] = useState(emptyFilters);
  const [page, setPage] = useState<LogPage>({ items: [], has_more: false });
  const [offset, setOffset] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [detail, setDetail] = useState<LogDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailError, setDetailError] = useState<string | null>(null);
  const queryVersion = useRef(0);
  const detailVersion = useRef(0);

  const queryLogs = useCallback(async (nextFilters: typeof emptyFilters, nextOffset: number) => {
    const version = ++queryVersion.current;
    ++detailVersion.current;
    setDetail(null);
    setDetailError(null);
    setDetailLoading(false);
    setError(null);
    if (!("__TAURI_INTERNALS__" in window)) {
      setError(t(locale, "models.tauriOnly"));
      return;
    }
    const from = nextFilters.from ? new Date(nextFilters.from) : null;
    const to = nextFilters.to ? new Date(nextFilters.to) : null;
    if ((from && Number.isNaN(from.getTime())) || (to && Number.isNaN(to.getTime())) || (from && to && from > to)) {
      setError(t(locale, "requestLogs.dateError"));
      return;
    }
    setLoading(true);
    try {
      const result = await invoke<LogPage>("query_model_request_logs", {
        query: { ...nextFilters, from: from?.toISOString() ?? null, to: to?.toISOString() ?? null, offset: nextOffset },
      });
      if (version !== queryVersion.current) return;
      setPage(result);
      setApplied(nextFilters);
      setOffset(nextOffset);
    } catch (cause) {
      if (version === queryVersion.current) setError(String(cause));
    } finally {
      if (version === queryVersion.current) setLoading(false);
    }
  }, [locale]);

  useEffect(() => {
    void queryLogs(emptyFilters, 0);
    return () => { ++queryVersion.current; ++detailVersion.current; };
  }, [queryLogs]);

  async function showDetail(id: string) {
    const version = ++detailVersion.current;
    setDetail(null);
    setDetailError(null);
    setDetailLoading(true);
    try {
      const result = await invoke<LogDetail>("model_request_log_detail", { id });
      if (version === detailVersion.current) setDetail(result);
    } catch (cause) {
      if (version === detailVersion.current) setDetailError(String(cause));
    } finally {
      if (version === detailVersion.current) setDetailLoading(false);
    }
  }

  return <>
    <div className="resilience-toggle-row">
      <span className="panel-title">{t(locale, "requestLogs.record")}</span>
      <button type="button" className={"browser-toggle" + (enabled ? " on" : "")} role="switch" aria-checked={enabled} aria-label={t(locale, "requestLogs.record")} disabled={disabled} onClick={() => onEnabledChange(!enabled)}>
        <span className="browser-toggle-knob" />
      </button>
    </div>
    <p className="mode-help">{t(locale, "requestLogs.hint")}</p>
    <form className="request-log-filters" onSubmit={(event) => { event.preventDefault(); void queryLogs(filters, 0); }}>
      {(["from", "to", "model", "endpoint"] as const).map((field) => <label className="field-label" key={field}>
        {t(locale, ("requestLogs." + field) as MessageKey)}
        <input type={field === "from" || field === "to" ? "datetime-local" : "text"} step={1} value={filters[field]} onChange={(event) => setFilters({ ...filters, [field]: event.target.value })} />
      </label>)}
      <label className="field-label">{t(locale, "requestLogs.status")}
        <select value={filters.status} onChange={(event) => setFilters({ ...filters, status: event.target.value })}>
          <option value="">{t(locale, "requestLogs.all")}</option>
          {Object.entries(statusKeys).map(([status, key]) => <option key={status} value={status}>{t(locale, key)}</option>)}
        </select>
      </label>
      <div className="request-log-actions">
        <button type="submit" className="primary-button" disabled={loading}>{t(locale, loading ? "requestLogs.loading" : "requestLogs.query")}</button>
        <button type="button" className="quiet-button" disabled={loading} onClick={() => { setFilters(emptyFilters); void queryLogs(emptyFilters, 0); }}>{t(locale, "requestLogs.reset")}</button>
      </div>
    </form>
    <p className="mode-help">{t(locale, "requestLogs.latest")}</p>
    {error ? <p role="alert" className="models-error">{error}</p> : null}
    <div className="request-log-table-wrap" aria-busy={loading}>
      {page.items.length ? <table className="request-log-table">
        <thead><tr>
          <th>{t(locale, "requestLogs.time")}</th><th>{t(locale, "requestLogs.model")}</th>
          <th>{t(locale, "requestLogs.endpoint")}</th><th>{t(locale, "requestLogs.status")}</th>
          <th>{t(locale, "requestLogs.http")}</th><th>{t(locale, "requestLogs.duration")}</th><th>{t(locale, "requestLogs.details")}</th>
        </tr></thead>
        <tbody>{page.items.map((item) => <tr key={item.id}>
          <td><time dateTime={item.created_at}>{new Date(item.created_at).toLocaleString(locale)}</time></td>
          <td>{item.model}</td><td>{item.endpoint}</td><td>{t(locale, statusKeys[item.status])}</td>
          <td>{item.http_status ?? "—"}</td><td>{item.duration_ms}</td>
          <td><button type="button" className="quiet-button" disabled={loading} onClick={() => void showDetail(item.id)}>{t(locale, "requestLogs.details")}</button></td>
        </tr>)}</tbody>
      </table> : !loading && !error ? <p className="mode-help">{t(locale, "requestLogs.empty")}</p> : null}
    </div>
    <div className="request-log-actions">
      <button type="button" className="quiet-button" disabled={loading || offset === 0} onClick={() => void queryLogs(applied, offset - 10)}>{t(locale, "requestLogs.previous")}</button>
      <span>{t(locale, "requestLogs.page", { page: offset / 10 + 1 })}</span>
      <button type="button" className="quiet-button" disabled={loading || !page.has_more} onClick={() => void queryLogs(applied, offset + 10)}>{t(locale, "requestLogs.next")}</button>
    </div>
    {detailLoading ? <p role="status">{t(locale, "requestLogs.loading")}</p> : null}
    {detailError ? <p role="alert" className="models-error">{detailError}</p> : null}
    {detail ? <section className="request-log-detail" aria-label={t(locale, "requestLogs.details")}>
      <div className="request-log-actions">
        <strong>{detail.model} · {t(locale, statusKeys[detail.status])}</strong>
        <button type="button" className="quiet-button" onClick={() => { ++detailVersion.current; setDetail(null); }}>{t(locale, "requestLogs.close")}</button>
      </div>
      <p>{new Date(detail.created_at).toLocaleString(locale)} · POST {detail.endpoint}</p>
      <p>{t(locale, "requestLogs.http")}: {detail.http_status ?? "—"} · {detail.duration_ms} ms · {detail.response_content_type}</p>
      {detail.error ? <pre className="models-error">{detail.error}</pre> : null}
      {detail.truncated ? <p role="status">{t(locale, "requestLogs.truncated")}</p> : null}
      <h3>{t(locale, "requestLogs.requestHeaders")}</h3>
      <pre>{detail.request_headers?.length ? detail.request_headers.map((header) => header.name + ": " + header.value).join("\n") : t(locale, "requestLogs.headersUnavailable")}</pre>
      <h3>{t(locale, "requestLogs.requestBody")}</h3><pre>{detail.request_body || "—"}</pre>
      <h3>{t(locale, "requestLogs.responseBody")}</h3><pre>{detail.response_body || "—"}</pre>
    </section> : null}
  </>;
}
