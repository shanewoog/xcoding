export const JSON_RPC_VERSION = "2.0" as const;

export type Mode = "ask" | "auto-edit" | "full-auto";
export type SessionStatus =
  | "created"
  | "running"
  | "need_user"
  | "done"
  | "failed"
  | "cancelled";
export type MessageRole = "system" | "user" | "assistant" | "tool";
export type ToolName = "list_dir" | "read_file" | "search_code" | "load_skill" | "apply_patch" | "run_command" | "git_status" | "git_diff" | "git_log" | "git_show" | "git_add" | "git_commit" | "git_push" | "git_fetch" | "git_pull" | "mcp";

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

export interface LocalPluginItem {
  id: string;
  kind: "mcp" | "skill";
  name: string;
  description: string;
  source: "user" | "workspace";
  enabled: boolean;
  status: string;
  tool_count?: number;
  env_keys?: string[];
}

export interface ProviderAuthStatus {
  ready: boolean;
  has_api_key: boolean;
  base_url: string;
  key_hint?: string;
  message: string;
}

/** One model entry from an OpenAI-compatible /models response. */
export interface ProviderModel {
  id: string;
  owned_by?: string;
}

/** Result of listing models from the configured cloud provider. */
export interface ListModelsResult {
  models: ProviderModel[];
  base_url: string;
}

/** One OpenAI-compatible cloud provider endpoint in Desktop settings. */
export type ProviderWireApi = "chat_completions" | "responses";
export type ProviderTrustLevel = "local" | "official" | "relay";

/** How provider HTTP traffic reaches the network. */
export type HttpProxyMode = "off" | "system" | "custom";

/** One credential inside a provider key pool. */
export interface ProviderApiKey {
  /** Stable id used by rotation state and log labels. */
  id: string;
  /** Optional human label, for example the account this key belongs to. */
  label?: string;
  /** Full API key. Never log this value. */
  key: string;
  /** Relative share in the weighted rotation. 0 keeps the key out. */
  weight?: number;
  /** Disabled keys stay out of the rotation without losing their configuration. */
  enabled?: boolean;
}

/** Rotation health for one configured credential, as shown in the settings view. */
export interface ProviderKeyStatus {
  provider_id: string;
  provider_name: string;
  key_id: string;
  label: string;
  /** Masked tail of the key. Never contains the full secret. */
  key_hint: string;
  weight: number;
  enabled: boolean;
  /** One of ready | rejected | rate_limited | unstable | disabled. */
  state: string;
  /** Remaining cooldown in seconds, only while the key is blocked. */
  cooldown_secs?: number;
  success_count: number;
  failure_count: number;
}

export interface CloudProviderConfig {
  id: string;
  /** Display name shown in the Desktop provider manager. */
  name: string;
  /** OpenAI-compatible API host without a trailing /v1 suffix. */
  base_url: string;
  /** HTTP request/stream protocol used by this provider. */
  wire_api?: ProviderWireApi;
  /** Trust boundary used for sensitive-data routing and fallback isolation. */
  trust_level?: ProviderTrustLevel;
  /** Full API key when configured. Never log this value. */
  api_key?: string;
  /** Weighted key pool for this provider. Falls back to api_key when empty. */
  api_keys?: ProviderApiKey[];
}

/**
 * One provider share for a single logical model. Several routes of the same
 * model spread that model's turns across independent providers, and each route
 * may rename the model for its own endpoint.
 */
export interface ModelRoute {
  /** Id of a provider in UserConfig.providers. */
  provider_id: string;
  /** Relative share in the model's weighted provider rotation. 0 keeps it out. */
  weight?: number;
  /** Disabled routes stay configured but are excluded from the rotation. */
  enabled?: boolean;
  /** Upstream model id to request instead of the logical model name. */
  model_override?: string;
}

/** Runtime state of one model route, aggregated over that provider's credentials. */
export interface ModelRouteStatus {
  /** Normalized model id this route belongs to. */
  model: string;
  provider_id: string;
  provider_name: string;
  weight: number;
  enabled: boolean;
  /** Upstream model id actually requested from this provider. */
  effective_model: string;
  /**
   * One of ready | disabled | unknown_provider | no_credential | blocked |
   * cooling_down | trust_mismatch.
   */
  state: string;
  /** Credentials of this provider currently usable for the route. */
  usable_key_count: number;
  success_count: number;
  failure_count: number;
}

/** Vision delegate configuration for models without native vision support. */
export interface VisionDelegateConfig {
  /** Whether vision delegation is enabled. */
  enabled: boolean;
  /** Provider id used for vision calls. */
  provider_id: string;
  /** Vision model id, for example gpt-4o. */
  model: string;
  /** Timeout for vision model calls in seconds. */
  timeout_seconds: number;
}

/** Model capability flags for vision support detection. */
export interface ModelCapabilities {
  /** Whether the model natively supports image inputs. */
  supports_vision: boolean;
}

/** User-level preferences stored under ~/.xcoding/config.json */
export interface UserConfig {
  locale: string;
  mode: Mode;
  /** Extra instructions appended to the system prompt on every turn. */
  custom_instructions?: string;
  /** Reply tone: default | pragmatic | friendly | concise | teaching. */
  personality?: string;
  /** Whether XCoding records short workspace facts after each turn. */
  local_memory_enabled?: boolean;
  /** Whether memory recording also runs on turns that used MCP tools. */
  tool_memory_enabled?: boolean;
  /** Technical provider id used by sessions (currently always openai-compatible). */
  provider: string;
  model: string;
  /** Reasoning effort for compatible models: none | low | medium | high. */
  reasoning_effort?: string;
  /** Retries after the initial failed request for one provider before XCoding tries a backup provider. */
  max_provider_retries?: number;
  /** Whether XCoding may switch to another configured provider after the active provider fails. */
  provider_fallback_enabled?: boolean;
  /** Maximum model/tool interaction rounds for one user turn before the agent stops. */
  max_tool_rounds?: number;
  /** Consecutive failed turns that open a provider circuit. */
  circuit_failure_threshold?: number;
  /** Maximum wait for the first stream event, including initial response setup. */
  stream_first_event_timeout_secs?: number;
  /** Maximum gap between stream events after the first event is received. */
  stream_idle_timeout_secs?: number;
  /** Reserved for future non-streaming provider calls; streaming chat does not use this yet. */
  non_stream_timeout_secs?: number;
  /** Successful half-open turns required before a provider circuit closes. */
  circuit_recovery_success_threshold?: number;
  /** Seconds a tripped provider circuit remains open before a half-open probe. */
  circuit_recovery_wait_secs?: number;
  /** Failure rate percentage that can open a circuit after enough samples. */
  circuit_error_rate_threshold_percent?: number;
  /** Minimum completed provider turns for failure-rate circuit evaluation. */
  circuit_min_request_count?: number;
  /** Active OpenAI-compatible API host without a trailing /v1 suffix. */
  base_url: string;
  api_key?: string;
  /** Managed cloud providers. At most one is active via active_provider_id. */
  providers?: CloudProviderConfig[];
  /** Id of the currently enabled cloud provider in providers. */
  active_provider_id?: string;
  /** Last opened project path (agent root). */
  last_workspace_root?: string;
  /** Parent directory where new projects are created. */
  workspace_home?: string;
  /** Project paths removed from the Desktop project area (folders stay on disk). */
  hidden_project_paths?: string[];
  /** Skip high-risk confirmation only for tightly constrained PowerShell loopback API requests. */
  skip_local_api_confirmation?: boolean;
  /** Per-model context window overrides keyed by normalized (trimmed, lowercased) model id. */
  model_context_windows?: Record<string, number>;
  /** Percentage of the configured model context window at which pre-compaction starts. */
  context_compaction_threshold_percent?: number;
  /** Describes images with a second model when the session model cannot read them. */
  vision_delegate?: VisionDelegateConfig;
  /** Per-model capability overrides keyed by normalized (trimmed, lowercased) model id. */
  model_capabilities?: Record<string, ModelCapabilities>;
  /** Per-model provider routes keyed by normalized (trimmed, lowercased) model id. */
  model_routes?: Record<string, ModelRoute[]>;
  /** How provider HTTP requests reach the network: off | system | custom. */
  http_proxy_mode?: HttpProxyMode;
  /** Proxy URL used when http_proxy_mode is custom, e.g. http://127.0.0.1:10808. */
  http_proxy_url?: string;
}

export interface ProjectDir {
  path: string;
  dir_name: string;
  title: string;
}

export interface CreateProjectParams {
  workspace_home: string;
  name: string;
}

export interface CreateProjectResult {
  project: ProjectDir;
}

export interface ImportProjectParams {
  workspace_home: string;
  source_path: string;
}

export interface ImportProjectResult {
  project: ProjectDir;
  already_existed: boolean;
  copied: boolean;
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

export interface GetSessionDetailParams {
  session_id: string;
}

export interface GetSessionDetailResult {
  detail: SessionDetail;
}

export interface ReplaySessionParams {
  session_id: string;
}

export interface ReplayStep {
  kind: string;
  summary: string;
  tool_name?: ToolName;
  success?: boolean;
}

export interface ReplaySessionResult {
  session: Session;
  events: PersistedSessionEvent[];
  steps: ReplayStep[];
}


export interface WorkspaceConfig {
  workspace_root: string;
  mode: Mode;
  provider: string;
  model: string;
  /** Extra auto-edit command allowlist patterns from `.xcoding/command-allowlist`. */
  command_allowlist?: string[];
  /** Workspace command denylist patterns from `.xcoding/command-denylist`. */
  command_denylist?: string[];
  updated_at: string;
}

export type FileChangeKind = "created" | "modified" | "deleted";

export interface FileChangeSummary {
  path: string;
  kind: FileChangeKind;
  lines_added: number;
  lines_removed: number;
}

export interface TaskSummary {
  changed_files: string[];
  file_changes?: FileChangeSummary[];
  commands_run: number;
  commands_succeeded: number;
  commands_failed: number;
  lines_added?: number;
  lines_removed?: number;
  git_branch?: string;
  git_status?: string;
  git_diff?: string;
}

export interface GetConfigParams {
  workspace_root: string;
}

export interface GetConfigResult {
  config: WorkspaceConfig;
}

export interface SetConfigParams {
  workspace_root: string;
  mode: Mode;
  provider: string;
  model: string;
  /** When set, rewrites `.xcoding/command-allowlist`. Omit to leave the file unchanged. */
  command_allowlist?: string[];
  /** When set, rewrites `.xcoding/command-denylist`. Omit to leave the file unchanged. */
  command_denylist?: string[];
}

export interface SetConfigResult {
  config: WorkspaceConfig;
}

export interface ChatImageAttachment {
  /** MIME type, e.g. image/png */
  mime_type: string;
  /** Raw base64 payload without data: prefix */
  data_base64: string;
  /** Optional original file name for display */
  name?: string;
}

export interface ChatParams {
  workspace_root: string;
  message: string;
  mode?: Mode;
  provider?: string;
  model?: string;
  title?: string;
  /** Continue an existing finished session instead of creating a new one. */
  session_id?: string;
  /** Optional image attachments for vision-capable models. */
  images?: ChatImageAttachment[];
}

export interface ChatResult {
  session: Session;
  message?: Message;
}

export interface RollbackRestorePointParams {
  session_id: string;
  restore_point_id: string;
}

export interface RollbackRestorePointResult {
  session: Session;
  restore_point: RestorePoint;
}

export interface CancelSessionParams {
  session_id: string;
  /** Optional assistant text already shown in the UI; persisted on cancel/steer. */
  partial_assistant?: string;
}

export interface CancelSessionResult {
  session: Session;
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

export interface RestorePoint {
  id: string;
  session_id: string;
  path: string;
  original_text?: string;
  applied_text?: string;
  created_at: string;
}

export interface PersistedSessionEvent {
  id: string;
  session_id: string;
  event: SessionEvent;
  created_at: string;
}

export interface SessionDetail {
  session: Session;
  messages: Message[];
  pending_actions: PendingAction[];
  restore_points: RestorePoint[];
  events: PersistedSessionEvent[];
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
      type: "restore_point_rolled_back";
      session_id: string;
      restore_point: RestorePoint;
      summary: string;
    }
  | {
      type: "session_cancelled";
      session_id: string;
      message: string;
    }
  | {
      type: "task_completed";
      session_id: string;
      summary: TaskSummary;
    }
  | {
      type: "retrying";
      session_id: string;
      attempt: number;
      max_attempts: number;
      message: string;
    }
  | {
      type: "stream_reset";
      session_id: string;
      discarded_chars: number;
      reason: string;
    }
  | {
      type: "model_call";
      session_id: string;
      provider: string;
      /** Configured id of the provider that served the request. */
      provider_id?: string;
      /** Display name of the provider that served the request. */
      provider_name?: string;
      /** Masked credential tail, never the credential itself. */
      key_hint?: string;
      model: string;
      /** Model identifier actually sent to the upstream endpoint. */
      effective_model?: string;
      endpoint: string;
      purpose: string;
      round: number;
      attempt: number;
      max_attempts: number;
      success: boolean;
      output_chars: number;
      tool_calls: number;
      error?: string;
      /** Model identifier reported back by the upstream, when it sent one. */
      model_reported?: string;
    }
  | {
      type: "vision_delegate_start";
      session_id: string;
      image_count: number;
      delegate_model: string;
      /** True when the images come from an earlier turn, not the message just sent. */
      historical?: boolean;
    }
  | {
      type: "vision_delegate_success";
      session_id: string;
      image_count: number;
      description_length: number;
    }
  | {
      type: "vision_delegate_failed";
      session_id: string;
      image_count: number;
      error: string;
    }
  | {
      type: "vision_descriptions_applied";
      session_id: string;
      image_count: number;
      /** Characters spent on descriptions of attachments from earlier turns. */
      historical_chars: number;
      /** True when an earlier description was clipped or omitted for budget. */
      truncated: boolean;
    }
  | {
      type: "context_compacted";
      session_id: string;
      compacted_message_count: number;
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
