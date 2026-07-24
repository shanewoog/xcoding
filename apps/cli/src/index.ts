#!/usr/bin/env node

import { existsSync } from "node:fs";
import { mkdir, readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { StdioRpcClient } from "@xcoding/client";
import type {
  CancelSessionResult,
  ChatParams,
  ChatResult,
  CreateSessionParams,
  CreateSessionResult,
  GetConfigResult,
  GetSessionDetailResult,
  ReplaySessionResult,
  ListSessionsResult,
  PingResult,
  ProviderAuthStatus,
  ResolveActionParams,
  ResolveActionResult,
  RollbackRestorePointResult,
  SessionEvent,
  TaskSummary,
  SetConfigParams,
  SetConfigResult,
} from "@xcoding/protocol";

const currentDirectory = dirname(fileURLToPath(import.meta.url));
const defaultServerPath = resolve(
  process.env.XCODING_SERVER_PATH ??
    resolve(currentDirectory, "../../../target/debug/xcoding-server.exe"),
);
const optionNames = new Set([
  "--workspace",
  "--server",
  "--provider",
  "--model",
  "--title",
  "--mode",
  "--session",
  "--command-allowlist",
  "--command-denylist",
]);

type CliMode = "ask" | "auto-edit";

function parseModeOption(value: string | undefined): CliMode | undefined {
  if (value === undefined) return undefined;
  if (value === "ask" || value === "auto-edit") return value;
  throw new Error(`invalid mode: ${value} (expected ask or auto-edit)`);
}

function parseCommandAllowlistOption(value: string): string[] {
  return value
    .split(/[\n,]/)
    .map((item) => item.trim())
    .filter((item) => item.length > 0 && !item.startsWith("#"));
}

function parseCommandDenylistOption(value: string): string[] {
  return parseCommandAllowlistOption(value);
}

async function main(): Promise<void> {
  await loadDotEnvFiles();
  const commandArguments = process.argv.slice(2);
  const [command, ...args] = commandArguments[0] === "--" ? commandArguments.slice(1) : commandArguments;

  if (!command || command === "help" || command === "--help" || command === "-h") {
    printUsage();
    return;
  }

  const invocationCwd = process.env.INIT_CWD ?? process.cwd();
  const workspace = resolve(invocationCwd, option(args, "--workspace") ?? ".");
  const databasePath = resolve(workspace, ".xcoding", "xcoding.db");
  await mkdir(dirname(databasePath), { recursive: true });

  const client = await StdioRpcClient.start({
    serverPath: option(args, "--server") ?? defaultServerPath,
    databasePath,
  });

  try {
    switch (command) {
      case "ping": {
        const result = await client.request<PingResult>("system.ping", {});
        console.log(`XCoding core ${result.version}: ${result.ok ? "ready" : "unavailable"}`);
        return;
      }
      case "auth":
      case "provider": {
        const status = await client.request<ProviderAuthStatus>("provider.status", {});
        console.log(JSON.stringify(status, null, 2));
        if (!status.ready) {
          process.exitCode = 2;
        }
        return;
      }
      case "doctor": {
        await runDoctorCommand(client, workspace, option(args, "--server") ?? defaultServerPath);
        return;
      }
      case "session":
        await runSessionCommand(client, workspace, args);
        return;
      case "config":
        await runConfigCommand(client, workspace, args);
        return;
      case "chat":
        await runChatCommand(client, workspace, args);
        return;
      default:
        throw new Error(`unknown command: ${command}`);
    }
  } finally {
    await client.close();
  }
}

async function runConfigCommand(
  client: StdioRpcClient,
  workspace: string,
  args: string[],
): Promise<void> {
  const [subcommand] = args;
  if (subcommand === "show") {
    const result = await client.request<GetConfigResult>("config.get", { workspace_root: workspace });
    console.log(JSON.stringify(result.config, null, 2));
    return;
  }
  if (subcommand === "set") {
    const mode = parseModeOption(option(args, "--mode"));
    const provider = option(args, "--provider");
    const model = option(args, "--model");
    const commandAllowlistRaw = option(args, "--command-allowlist");
    const commandDenylistRaw = option(args, "--command-denylist");
    if (!mode && !provider && !model && commandAllowlistRaw === undefined && commandDenylistRaw === undefined) {
      throw new Error(
        "expected at least one of `--mode`, `--provider`, `--model`, `--command-allowlist`, or `--command-denylist`",
      );
    }
    const current = await client.request<GetConfigResult>("config.get", { workspace_root: workspace });
    const params: SetConfigParams = {
      workspace_root: workspace,
      mode: mode ?? current.config.mode,
      provider: provider ?? current.config.provider,
      model: model ?? current.config.model,
    };
    if (commandAllowlistRaw !== undefined) {
      params.command_allowlist = parseCommandAllowlistOption(commandAllowlistRaw);
    }
    if (commandDenylistRaw !== undefined) {
      params.command_denylist = parseCommandDenylistOption(commandDenylistRaw);
    }
    const result = await client.request<SetConfigResult>("config.set", params);
    console.log(JSON.stringify(result.config, null, 2));
    return;
  }
  throw new Error("expected `config show` or `config set`");
}

async function runSessionCommand(
  client: StdioRpcClient,
  workspace: string,
  args: string[],
): Promise<void> {
  const [subcommand] = args;

  switch (subcommand) {
    case "create": {
      const params: CreateSessionParams = {
        workspace_root: workspace,
        title: option(args, "--title"),
        mode: parseModeOption(option(args, "--mode")),
        provider: option(args, "--provider"),
        model: option(args, "--model"),
      };
      const result = await client.request<CreateSessionResult>("session.create", withoutUndefined(params));
      console.log(JSON.stringify(result.session, null, 2));
      return;
    }
    case "list": {
      const result = await client.request<ListSessionsResult>("session.list", {
        workspace_root: workspace,
      });
      if (result.sessions.length === 0) {
        console.log("No sessions found.");
        return;
      }
      for (const session of result.sessions) {
        console.log(`${session.id}\t${session.status}\t${session.title ?? "Untitled"}`);
      }
      return;
    }
    case "show": {
      const sessionId = requiredArgument(args[1], "expected `session show <session-id>`");
      const result = await client.request<GetSessionDetailResult>("session.detail", { session_id: sessionId });
      console.log(JSON.stringify(result.detail, null, 2));
      return;
    }
    case "replay": {
      const sessionId = requiredArgument(args[1], "expected `session replay <session-id>`");
      const result = await client.request<ReplaySessionResult>("session.replay", { session_id: sessionId });
      console.log(JSON.stringify({
        session: result.session,
        steps: result.steps,
        event_count: result.events.length,
      }, null, 2));
      return;
    }
    case "approve":
    case "reject": {
      const sessionId = requiredArgument(args[1], `expected session ${subcommand} <session-id> <action-id>`);
      const actionId = requiredArgument(args[2], `expected session ${subcommand} <session-id> <action-id>`);
      await runResolveAction(client, {
        session_id: sessionId,
        action_id: actionId,
        approved: subcommand === "approve",
      });
      return;
    }
    case "rollback": {
      const sessionId = requiredArgument(args[1], "expected `session rollback <session-id> <restore-point-id>`");
      const restorePointId = requiredArgument(args[2], "expected `session rollback <session-id> <restore-point-id>`");
      const result = await runWithEvents<RollbackRestorePointResult>(client, "session.rollback", {
        session_id: sessionId,
        restore_point_id: restorePointId,
      });
      console.log(`Restored ${result.restore_point.path} in session ${result.session.id}: ${result.session.status}`);
      return;
    }
    case "cancel": {
      const sessionId = requiredArgument(args[1], "expected `session cancel <session-id>`");
      const result = await runWithEvents<CancelSessionResult>(client, "session.cancel", { session_id: sessionId });
      console.log(`Session ${result.session.id}: ${result.session.status}`);
      return;
    }
    case "summary": {
      const sessionId = requiredArgument(args[1], "expected `session summary <session-id>`");
      const result = await client.request<GetSessionDetailResult>("session.detail", { session_id: sessionId });
      const summary = latestTaskSummary(result.detail.events);
      if (!summary) {
        console.log(`Session ${result.detail.session.id}: no task summary yet (status ${result.detail.session.status}).`);
        return;
      }
      console.log(formatTaskSummary(summary, result.detail.session.id));
      return;
    }
    default:
      throw new Error("expected `session create`, `session list`, `session show`, `session summary`, `session replay`, `session approve`, `session reject`, `session rollback`, or `session cancel`");

  }
}

async function runChatCommand(
  client: StdioRpcClient,
  workspace: string,
  args: string[],
): Promise<void> {
  const message = positionalArguments(args).join(" ").trim();
  if (!message) {
    throw new Error("expected a chat message");
  }

  const params: ChatParams = {
    workspace_root: workspace,
    message,
    title: option(args, "--title"),
    mode: parseModeOption(option(args, "--mode")),
    provider: option(args, "--provider"),
    model: option(args, "--model"),
    session_id: option(args, "--session"),
  };
  const result = await runWithEvents<ChatResult>(client, "session.chat", withoutUndefined(params));
  console.log(`Session ${result.session.id}: ${result.session.status}`);
}

async function runResolveAction(client: StdioRpcClient, params: ResolveActionParams): Promise<void> {
  const result = await runWithEvents<ResolveActionResult>(client, "session.resolve", params);
  console.log(`Session ${result.session.id}: ${result.session.status}`);
}

async function runWithEvents<TResult>(
  client: StdioRpcClient,
  method: string,
  params: object,
): Promise<TResult> {
  let receivedText = false;
  const unsubscribe = client.onNotification((notification) => {
    if (notification.method !== "session.event") return;
    const event = notification.params as SessionEvent;
    if (event.type === "text_delta") receivedText = true;
    printEvent(event);
  });

  try {
    return await client.request<TResult>(method, params);
  } finally {
    if (receivedText) process.stdout.write("\n");
    unsubscribe();
  }
}


function formatGitApprovalDetail(
  toolCall: { name?: string; arguments?: Record<string, unknown> } | null | undefined,
): string | null {
  if (!toolCall?.name) return null;
  const args = toolCall.arguments ?? {};
  const asString = (value: unknown) =>
    typeof value === "string" && value.trim() ? value : null;
  const asStringArray = (value: unknown) =>
    Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
  switch (toolCall.name) {
    case "git_add": {
      const paths = asStringArray(args.paths);
      return paths.length > 0 ? paths.join(", ") : "<paths>";
    }
    case "git_commit": {
      const message = asString(args.message) ?? "<message>";
      return message.split(/\r?\n/)[0] ?? message;
    }
    case "git_push":
    case "git_fetch":
    case "git_pull": {
      const remote = asString(args.remote) ?? "origin";
      const branch =
        asString(args.branch) ?? (toolCall.name === "git_fetch" ? "<all>" : "<current-branch>");
      if (toolCall.name === "git_pull") {
        const ffOnly = typeof args.ff_only === "boolean" ? args.ff_only : true;
        return `${remote} ${branch} (${ffOnly ? "ff-only" : "no-rebase"})`;
      }
      if (toolCall.name === "git_push" && typeof args.set_upstream === "boolean") {
        return `${remote} ${branch} (set-upstream=${args.set_upstream})`;
      }
      return `${remote} ${branch}`;
    }
    default:
      return null;
  }
}

function printEvent(event: SessionEvent): void {
  switch (event.type) {
    case "text_delta":
      process.stdout.write(event.delta);
      return;
    case "plan":
      process.stderr.write(`Plan:\n${event.steps.map((step) => `- ${step.description}`).join("\n")}\n`);
      return;
    case "tool_start":
      process.stderr.write(`> ${event.summary}\n`);
      return;
    case "tool_end":
      process.stderr.write(`${event.success ? "done" : "failed"}: ${event.summary}\n`);
      if (
        !event.success &&
        typeof event.summary === "string" &&
        event.summary.toLowerCase().includes("patch conflict")
      ) {
        process.stderr.write(
          "HINT: re-read the file and retry apply_patch with updated old_text.\n",
        );
      }
      return;
    case "patch_preview":
      process.stderr.write(`Patch: ${event.preview.path}\n`);
      return;
    case "approval_requested": {
      process.stderr.write(`Approval required: ${event.action.id} (${event.summary})\n`);
      if (typeof event.summary === "string" && event.summary.toUpperCase().includes("HIGH-RISK")) {
        const toolName = event.action.tool_call?.name ?? "";
        const isGitWrite = [
          "git_add",
          "git_commit",
          "git_push",
          "git_fetch",
          "git_pull",
        ].includes(toolName);
        if (isGitWrite) {
          process.stderr.write(
            "WARNING: HIGH-RISK git operation — review carefully before approving.\n",
          );
          const detail = formatGitApprovalDetail(event.action.tool_call);
          if (detail) process.stderr.write(`Git: ${detail}\n`);
        } else {
          process.stderr.write("WARNING: HIGH-RISK command — review carefully before approving.\n");
          const args = event.action.tool_call?.arguments;
          if (args && typeof args === "object" && "executable" in args) {
            const executable = typeof args.executable === "string" ? args.executable : "<command>";
            const argList = Array.isArray(args.args)
              ? args.args.filter((item) => typeof item === "string").join(" ")
              : "";
            process.stderr.write(`Command: ${argList ? `${executable} ${argList}` : executable}\n`);
          }
        }
      }
      return;
    }
    case "restore_point_rolled_back":
      process.stderr.write(`Restored: ${event.restore_point.path}\n`);
      return;
    case "session_cancelled":
      process.stderr.write(`${event.message}\n`);
      return;
    case "task_completed": {
      process.stderr.write(`${formatTaskSummary(event.summary)}\n`);
      return;
    }
    case "error":
      process.stderr.write(`Error: ${event.message}\n`);
      return;
    case "message_completed":
      return;
  }
}

function requiredArgument(value: string | undefined, message: string): string {
  if (!value) throw new Error(message);
  return value;
}

function option(args: string[], name: string): string | undefined {
  const index = args.indexOf(name);
  if (index < 0) return undefined;

  const value = args[index + 1];
  if (!value || value.startsWith("--")) {
    throw new Error(`expected a value after ${name}`);
  }
  return value;
}

function positionalArguments(args: string[]): string[] {
  const values: string[] = [];
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (optionNames.has(argument)) {
      index += 1;
      continue;
    }
    if (argument.startsWith("--")) throw new Error(`unknown option: ${argument}`);
    values.push(argument);
  }
  return values;
}

function withoutUndefined(value: object): Record<string, unknown> {
  return Object.fromEntries(Object.entries(value).filter(([, entry]) => entry !== undefined));
}


async function loadDotEnvFiles(): Promise<void> {
  const candidates: string[] = [];
  if (process.env.INIT_CWD) {
    candidates.push(resolve(process.env.INIT_CWD, ".env"));
  }
  candidates.push(resolve(process.cwd(), ".env"));
  candidates.push(resolve(currentDirectory, "../../../.env"));

  const seen = new Set<string>();
  for (const path of candidates) {
    if (seen.has(path) || !existsSync(path)) {
      continue;
    }
    seen.add(path);
    const text = await readFile(path, "utf8");
    for (const rawLine of text.split(/\r?\n/)) {
      const line = rawLine.trim();
      if (!line || line.startsWith("#")) {
        continue;
      }
      const separator = line.indexOf("=");
      if (separator <= 0) {
        continue;
      }
      const key = line.slice(0, separator).trim();
      let value = line.slice(separator + 1).trim();
      if (
        (value.startsWith("\"") && value.endsWith("\"")) ||
        (value.startsWith("'") && value.endsWith("'"))
      ) {
        value = value.slice(1, -1);
      }
      if (process.env[key] === undefined) {
        process.env[key] = value;
      }
    }
  }
}

async function runDoctorCommand(
  client: StdioRpcClient,
  workspace: string,
  serverPath: string,
): Promise<void> {
  const checks: Array<{ name: string; ok: boolean; detail: string }> = [];

  checks.push({
    name: "workspace",
    ok: existsSync(workspace),
    detail: workspace,
  });

  checks.push({
    name: "server_binary",
    ok: existsSync(serverPath),
    detail: serverPath,
  });

  try {
    const ping = await client.request<PingResult>("system.ping", {});
    checks.push({
      name: "core_rpc",
      ok: Boolean(ping.ok),
      detail: "version " + ping.version,
    });
  } catch (error) {
    checks.push({
      name: "core_rpc",
      ok: false,
      detail: error instanceof Error ? error.message : String(error),
    });
  }

  try {
    const status = await client.request<ProviderAuthStatus>("provider.status", {});
    checks.push({
      name: "provider_auth",
      ok: Boolean(status.ready),
      detail: status.ready
        ? status.base_url + " key=" + (status.key_hint ?? "set")
        : status.message,
    });
  } catch (error) {
    checks.push({
      name: "provider_auth",
      ok: false,
      detail: error instanceof Error ? error.message : String(error),
    });
  }

  try {
    const config = await client.request<GetConfigResult>("config.get", {
      workspace_root: workspace,
    });
    checks.push({
      name: "workspace_config",
      ok: true,
      detail:
        "mode=" +
        config.config.mode +
        " provider=" +
        config.config.provider +
        " model=" +
        config.config.model +
        " allowlist=" +
        (config.config.command_allowlist?.length ?? 0) +
        " denylist=" +
        (config.config.command_denylist?.length ?? 0),
    });
  } catch (error) {
    checks.push({
      name: "workspace_config",
      ok: false,
      detail: error instanceof Error ? error.message : String(error),
    });
  }

  let gitOk = false;
  let gitDetail = "git not found on PATH";
  try {
    const { spawnSync } = await import("node:child_process");
    const result = spawnSync("git", ["--version"], { encoding: "utf8" });
    gitOk = result.status === 0;
    gitDetail = (result.stdout || result.stderr || "").trim() || gitDetail;
  } catch (error) {
    gitDetail = error instanceof Error ? error.message : String(error);
  }
  checks.push({ name: "git", ok: gitOk, detail: gitDetail });

  try {
    const mcpPath = join(workspace, ".xcoding", "mcp.json");
    if (!existsSync(mcpPath)) {
      checks.push({
        name: "mcp_config",
        ok: true,
        detail: "no .xcoding/mcp.json (MCP disabled)",
      });
    } else {
      const raw = await readFile(mcpPath, "utf8");
      const parsed = JSON.parse(raw) as {
        mcpServers?: Record<string, { enabled?: boolean }>;
      };
      const servers = parsed.mcpServers ?? {};
      const names = Object.keys(servers);
      const enabled = names.filter((name) => servers[name]?.enabled !== false).length;
      checks.push({
        name: "mcp_config",
        ok: true,
        detail: `servers=${names.length} enabled=${enabled}`,
      });
    }
  } catch (error) {
    checks.push({
      name: "mcp_config",
      ok: false,
      detail: error instanceof Error ? error.message : String(error),
    });
  }

  const report = {
    ready: checks.every((check) => check.ok),
    workspace,
    checks,
  };
  console.log(JSON.stringify(report, null, 2));
  if (!report.ready) {
    process.exitCode = 2;
  }
}

function latestTaskSummary(events: GetSessionDetailResult["detail"]["events"]): TaskSummary | null {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index].event;
    if (event.type === "task_completed") return event.summary;
  }
  return null;
}

function formatTaskSummary(summary: TaskSummary, sessionId?: string): string {
  const lines: string[] = [];
  if (sessionId) lines.push(`Session ${sessionId}`);
  const added = summary.lines_added ?? 0;
  const removed = summary.lines_removed ?? 0;
  lines.push(
    `Task complete: ${summary.changed_files.length} changed file(s), +${added}/-${removed} line(s); ` +
      `${summary.commands_succeeded}/${summary.commands_run} command(s) succeeded` +
      (summary.commands_failed ? `, ${summary.commands_failed} failed` : "") +
      ".",
  );
  const fileChanges = summary.file_changes ?? [];
  if (fileChanges.length > 0) {
    lines.push("Files:");
    for (const change of fileChanges) {
      lines.push(
        `  [${change.kind}] ${change.path} (+${change.lines_added}/-${change.lines_removed})`,
      );
    }
  } else if (summary.changed_files.length > 0) {
    lines.push(`Changed: ${summary.changed_files.join(", ")}`);
  }
  if (summary.git_branch) lines.push(`Git branch: ${summary.git_branch}`);
  if (summary.git_status) lines.push(`Git status:\n${summary.git_status}`);
  if (summary.git_diff) lines.push(`Git diff:\n${summary.git_diff}`);
  return lines.join("\n");
}

function printUsage(): void {
  console.log(`XCoding CLI

Usage:
  xcoding ping [--workspace <path>] [--server <path>]
  xcoding auth [--workspace <path>] [--server <path>]
  xcoding doctor [--workspace <path>] [--server <path>]
  xcoding config show [--workspace <path>]
  xcoding config set [--workspace <path>] [--mode ask|auto-edit] [--provider openai] [--model <model>] [--command-allowlist <patterns>] [--command-denylist <patterns>]
  xcoding session create [--workspace <path>] [--title <text>] [--mode ask|auto-edit]
  xcoding session list [--workspace <path>]
  xcoding session show <session-id> [--workspace <path>]
  xcoding session replay <session-id> [--workspace <path>]
  xcoding session approve <session-id> <action-id> [--workspace <path>]
  xcoding session reject <session-id> <action-id> [--workspace <path>]
  xcoding session rollback <session-id> <restore-point-id> [--workspace <path>]
  xcoding session cancel <session-id> [--workspace <path>]
  xcoding chat "<message>" [--workspace <path>] [--session <id>] [--mode ask|auto-edit] [--provider openai] [--model <model>]

Environment:
  OPENAI_API_KEY           API key for the OpenAI-compatible cloud provider
  XCODING_OPENAI_BASE_URL  Optional OpenAI-compatible API base URL
  XCODING_SERVER_PATH      Absolute path to the xcoding-server binary

Dotenv:
  Loads repository-root .env if present. Existing process env wins.

Mode policy:
  ask         Propose patches and commands; both need approval
  auto-edit   Apply ordinary patches and allowlisted safe commands automatically; high-risk and other commands need approval

Command allowlist:
  Workspace file .xcoding/command-allowlist extends the builtin auto-edit command allowlist.
  Patterns are one per line or comma-separated via --command-allowlist (exe or exe:subcommand).
  Shells/interpreters and destructive system commands cannot be allowlisted.

Command denylist:
  Workspace file .xcoding/command-denylist hard-denies matching commands (overrides allowlist).
  Patterns are one per line or comma-separated via --command-denylist (exe or exe:subcommand).
  Shells may be listed on the denylist. Hard-denied commands return structured tool errors and never ask for approval.
`);
}

main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});