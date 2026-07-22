#!/usr/bin/env node

import { mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
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
  ListSessionsResult,
  PingResult,
  ResolveActionParams,
  ResolveActionResult,
  RollbackRestorePointResult,
  SessionEvent,
  SetConfigParams,
  SetConfigResult,
} from "@xcoding/protocol";

const currentDirectory = dirname(fileURLToPath(import.meta.url));
const defaultServerPath = resolve(
  process.env.XCODING_SERVER_PATH ??
    resolve(currentDirectory, "../../../target/debug/xcoding-server.exe"),
);
const optionNames = new Set(["--workspace", "--server", "--provider", "--model", "--title", "--mode"]);

async function main(): Promise<void> {
  const commandArguments = process.argv.slice(2);
  const [command, ...args] = commandArguments[0] === "--" ? commandArguments.slice(1) : commandArguments;

  if (!command || command === "help" || command === "--help" || command === "-h") {
    printUsage();
    return;
  }

  const workspace = option(args, "--workspace") ?? process.cwd();
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
    const mode = option(args, "--mode");
    const provider = option(args, "--provider");
    const model = option(args, "--model");
    if (!mode && !provider && !model) {
      throw new Error("expected at least one of `--mode`, `--provider`, or `--model`");
    }
    const current = await client.request<GetConfigResult>("config.get", { workspace_root: workspace });
    const params: SetConfigParams = {
      workspace_root: workspace,
      mode: (mode ?? current.config.mode) as SetConfigParams["mode"],
      provider: provider ?? current.config.provider,
      model: model ?? current.config.model,
    };
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
        mode: option(args, "--mode") as CreateSessionParams["mode"],
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
    default:
      throw new Error("expected `session create`, `session list`, `session show`, `session approve`, `session reject`, `session rollback`, or `session cancel`");
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
    mode: option(args, "--mode") as ChatParams["mode"],
    provider: option(args, "--provider"),
    model: option(args, "--model"),
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
      return;
    case "patch_preview":
      process.stderr.write(`Patch: ${event.preview.path}\n`);
      return;
    case "approval_requested":
      process.stderr.write(`Approval required: ${event.action.id} (${event.summary})\n`);
      return;
    case "restore_point_rolled_back":
      process.stderr.write(`Restored: ${event.restore_point.path}\n`);
      return;
    case "session_cancelled":
      process.stderr.write(`${event.message}\n`);
      return;
    case "task_completed":
      process.stderr.write(
        `Task complete: ${event.summary.changed_files.length} changed file(s); ` +
        `${event.summary.commands_succeeded}/${event.summary.commands_run} command(s) succeeded.\n`,
      );
      return;
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

function printUsage(): void {
  console.log(`XCoding CLI

Usage:
  xcoding ping [--workspace <path>] [--server <path>]
  xcoding config show [--workspace <path>]
  xcoding config set [--workspace <path>] [--mode ask|auto-edit] [--provider openai] [--model <model>]
  xcoding session create [--workspace <path>] [--title <text>] [--mode ask|auto-edit]
  xcoding session list [--workspace <path>]
  xcoding session show <session-id> [--workspace <path>]
  xcoding session approve <session-id> <action-id> [--workspace <path>]
  xcoding session reject <session-id> <action-id> [--workspace <path>]
  xcoding session rollback <session-id> <restore-point-id> [--workspace <path>]
  xcoding session cancel <session-id> [--workspace <path>]
  xcoding chat "<message>" [--workspace <path>] [--provider openai] [--model <model>]

Environment:
  OPENAI_API_KEY           API key for the OpenAI-compatible cloud provider
  XCODING_OPENAI_BASE_URL  Optional OpenAI-compatible API base URL
  XCODING_SERVER_PATH      Absolute path to the xcoding-server binary
`);
}

main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});