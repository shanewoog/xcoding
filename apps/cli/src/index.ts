#!/usr/bin/env node

import { mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { StdioRpcClient } from "@xcoding/client";
import type {
  CreateSessionParams,
  CreateSessionResult,
  ListSessionsResult,
  PingResult,
} from "@xcoding/protocol";

const currentDirectory = dirname(fileURLToPath(import.meta.url));
const defaultServerPath = resolve(
  process.env.XCODING_SERVER_PATH ??
    resolve(currentDirectory, "../../../target/debug/xcoding-server.exe"),
);

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
      default:
        throw new Error(`unknown command: ${command}`);
    }
  } finally {
    await client.close();
  }
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
    default:
      throw new Error("expected `session create` or `session list`");
  }
}

function option(args: string[], name: string): string | undefined {
  const index = args.indexOf(name);
  if (index < 0) {
    return undefined;
  }

  const value = args[index + 1];
  if (!value || value.startsWith("--")) {
    throw new Error(`expected a value after ${name}`);
  }
  return value;
}

function withoutUndefined(value: object): Record<string, unknown> {
  return Object.fromEntries(Object.entries(value).filter(([, entry]) => entry !== undefined));
}

function printUsage(): void {
  console.log(`XCoding Phase 0 CLI

Usage:
  xcoding ping [--workspace <path>] [--server <path>]
  xcoding session create [--workspace <path>] [--title <text>] [--mode ask|auto-edit]
  xcoding session list [--workspace <path>]

Environment:
  XCODING_SERVER_PATH  Absolute path to the xcoding-server binary
`);
}

main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
