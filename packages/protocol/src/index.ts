export const JSON_RPC_VERSION = "2.0" as const;

export type Mode = "ask" | "auto-edit";
export type SessionStatus =
  | "created"
  | "running"
  | "need_user"
  | "done"
  | "failed"
  | "cancelled";

export interface Session {
  id: string;
  workspace_root: string;
  mode: Mode;
  provider: string;
  model: string;
  status: SessionStatus;
  created_at: string;
  updated_at: string;
  title?: string;
}

export interface PingResult {
  ok: boolean;
  version: string;
}

export interface CreateSessionParams {
  workspace_root: string;
  mode?: Mode;
  provider?: string;
  model?: string;
  title?: string;
}

export interface CreateSessionResult {
  session: Session;
}

export interface ListSessionsParams {
  workspace_root?: string;
}

export interface ListSessionsResult {
  sessions: Session[];
}

export interface JsonRpcRequest<TParams = unknown> {
  jsonrpc: typeof JSON_RPC_VERSION;
  id: number;
  method: string;
  params: TParams;
}

export interface JsonRpcSuccess<TResult = unknown> {
  jsonrpc: typeof JSON_RPC_VERSION;
  id: number;
  result: TResult;
}

export interface JsonRpcFailure {
  jsonrpc: typeof JSON_RPC_VERSION;
  id: number | null;
  error: {
    code: number;
    message: string;
    data?: unknown;
  };
}

export type JsonRpcResponse<TResult = unknown> = JsonRpcSuccess<TResult> | JsonRpcFailure;

export function isJsonRpcFailure(response: JsonRpcResponse): response is JsonRpcFailure {
  return "error" in response;
}
