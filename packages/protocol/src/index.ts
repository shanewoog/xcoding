export const JSON_RPC_VERSION = "2.0" as const;

export type Mode = "ask" | "auto-edit";
export type SessionStatus =
  | "created"
  | "running"
  | "need_user"
  | "done"
  | "failed"
  | "cancelled";
export type MessageRole = "system" | "user" | "assistant" | "tool";
export type ToolName = "list_dir" | "read_file" | "search_code" | "apply_patch" | "run_command";

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

export interface Message {
  id: string;
  session_id: string;
  role: MessageRole;
  content: string;
  created_at: string;
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

export interface ChatParams {
  workspace_root: string;
  message: string;
  mode?: Mode;
  provider?: string;
  model?: string;
  title?: string;
}

export interface ChatResult {
  session: Session;
  message?: Message;
}

export interface ResolveActionParams {
  session_id: string;
  action_id: string;
  approved: boolean;
}

export interface ResolveActionResult {
  session: Session;
  message?: Message;
}

export interface ToolCall {
  id: string;
  name: ToolName;
  arguments: Record<string, unknown>;
}

export type PendingActionStatus = "pending" | "approved" | "rejected";

export interface PendingAction {
  id: string;
  session_id: string;
  tool_call: ToolCall;
  status: PendingActionStatus;
  created_at: string;
  resolved_at?: string;
}

export interface PatchPreview {
  path: string;
  file_existed: boolean;
  old_text: string;
  new_text: string;
}

export interface PlanStep {
  id: string;
  description: string;
}

export type SessionEvent =
  | {
      type: "text_delta";
      session_id: string;
      delta: string;
    }
  | {
      type: "message_completed";
      session_id: string;
      message: Message;
    }
  | {
      type: "plan";
      session_id: string;
      steps: PlanStep[];
    }
  | {
      type: "tool_start";
      session_id: string;
      tool_call: ToolCall;
      summary: string;
    }
  | {
      type: "tool_end";
      session_id: string;
      tool_call: ToolCall;
      success: boolean;
      summary: string;
    }
  | {
      type: "patch_preview";
      session_id: string;
      preview: PatchPreview;
    }
  | {
      type: "approval_requested";
      session_id: string;
      action: PendingAction;
      summary: string;
    }
  | {
      type: "error";
      session_id: string;
      message: string;
    };

export interface JsonRpcRequest<TParams = unknown> {
  jsonrpc: typeof JSON_RPC_VERSION;
  id: number;
  method: string;
  params: TParams;
}

export interface JsonRpcNotification<TParams = unknown> {
  jsonrpc: typeof JSON_RPC_VERSION;
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

export function isJsonRpcNotification(value: unknown): value is JsonRpcNotification {
  return (
    typeof value === "object" &&
    value !== null &&
    "jsonrpc" in value &&
    (value as { jsonrpc?: unknown }).jsonrpc === JSON_RPC_VERSION &&
    "method" in value &&
    typeof (value as { method?: unknown }).method === "string" &&
    !("id" in value)
  );
}

export function isJsonRpcResponse(value: unknown): value is JsonRpcResponse {
  return (
    typeof value === "object" &&
    value !== null &&
    "jsonrpc" in value &&
    (value as { jsonrpc?: unknown }).jsonrpc === JSON_RPC_VERSION &&
    "id" in value &&
    ("result" in value || "error" in value)
  );
}
