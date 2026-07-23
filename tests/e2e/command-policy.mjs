import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdtemp, rm, writeFile, readFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync } from "node:fs";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const binaryName = process.platform === "win32" ? "xcoding-server.exe" : "xcoding-server";
const serverPath = resolve(repositoryRoot, "target/debug", binaryName);

function writeText(response, text) {
  response.write(`data: ${JSON.stringify({
    choices: [{ delta: { content: text } }],
  })}\n\n`);
  response.write("data: [DONE]\n\n");
  response.end();
}

function writeToolCall(response, toolCall) {
  response.write(`data: ${JSON.stringify({
    choices: [{
      delta: {
        tool_calls: [{
          index: 0,
          id: toolCall.id,
          type: "function",
          function: {
            name: toolCall.name,
            arguments: JSON.stringify(toolCall.arguments),
          },
        }],
      },
    }],
  })}\n\n`);
  response.write("data: [DONE]\n\n");
  response.end();
}

async function main() {
  if (!existsSync(serverPath)) {
    throw new Error("build xcoding-server first: cargo build -p xcoding-server");
  }

  const workspace = await mkdtemp(join(tmpdir(), "xcoding-cmd-policy-"));
  const databaseDirectory = await mkdtemp(join(tmpdir(), "xcoding-cmd-policy-db-"));
  const databasePath = join(databaseDirectory, "xcoding.db");
  await writeFile(join(workspace, "README.md"), "# command policy fixture\n", "utf8");

  const mock = await startMockProvider();
  const environment = {
    ...process.env,
    OPENAI_API_KEY: "e2e-test-key",
    XCODING_OPENAI_BASE_URL: `http://127.0.0.1:${mock.port}/v1`,
  };
  const rpc = startRpcClient({ databasePath, environment });

  try {
    // Hard-deny builtin under auto-edit: no approval, structured tool failure.
    mock.scenario = "hard-deny";
    const denied = await rpc.request("session.chat", {
      workspace_root: workspace,
      message: "Run a hard-denied format command.",
      model: "fixture-model",
      mode: "auto-edit",
    });
    assert.equal(denied.session.status, "done");
    assert.equal(
      eventsFor(rpc, denied.session.id).some((event) => event.type === "approval_requested"),
      false,
      "hard-denied commands must not request approval",
    );
    const blockedStart = eventsFor(rpc, denied.session.id).find(
      (event) => event.type === "tool_start" && event.tool_call?.name === "run_command",
    );
    assert.ok(blockedStart);
    assert.match(String(blockedStart.summary), /Blocked/i);
    const blockedEnd = eventsFor(rpc, denied.session.id).find(
      (event) => event.type === "tool_end" && event.tool_call?.name === "run_command",
    );
    assert.ok(blockedEnd);
    assert.equal(blockedEnd.success, false);
    assert.match(String(blockedEnd.summary), /command blocked by policy|command_policy|policy/i);

    // Workspace denylist via config.set blocks allowlisted cargo --version.
    const saved = await rpc.request("config.set", {
      workspace_root: workspace,
      mode: "auto-edit",
      provider: "openai",
      model: "fixture-model",
      command_denylist: ["cargo:--version"],
    });
    assert.deepEqual(saved.config.command_denylist, ["cargo:--version"]);
    const denyBody = await readFile(join(workspace, ".xcoding", "command-denylist"), "utf8");
    assert.match(denyBody, /cargo:--version/);

    const loaded = await rpc.request("config.get", { workspace_root: workspace });
    assert.deepEqual(loaded.config.command_denylist, ["cargo:--version"]);

    mock.scenario = "workspace-deny";
    const workspaceDenied = await rpc.request("session.chat", {
      workspace_root: workspace,
      message: "Run cargo --version despite allowlist.",
      model: "fixture-model",
      mode: "auto-edit",
    });
    assert.equal(workspaceDenied.session.status, "done");
    assert.equal(
      eventsFor(rpc, workspaceDenied.session.id).some((event) => event.type === "approval_requested"),
      false,
      "workspace denylist must hard-deny without approval",
    );
    const denyStart = eventsFor(rpc, workspaceDenied.session.id).find(
      (event) => event.type === "tool_start" && event.tool_call?.name === "run_command",
    );
    assert.ok(denyStart);
    assert.match(String(denyStart.summary), /Blocked/i);
    const denyEnd = eventsFor(rpc, workspaceDenied.session.id).find(
      (event) => event.type === "tool_end" && event.tool_call?.name === "run_command",
    );
    assert.ok(denyEnd);
    assert.equal(denyEnd.success, false);
    assert.match(String(denyEnd.summary), /denied_workspace_denylist|command blocked by policy|workspace denylist/i);

    console.log("Command policy (denylist + hard-deny) checks passed.");
  } finally {
    await rpc.close();
    await mock.close();
    await rm(workspace, { recursive: true, force: true });
    await rm(databaseDirectory, { recursive: true, force: true });
  }
}

function eventsFor(rpc, sessionId) {
  return rpc.events.filter((event) => event.session_id === sessionId);
}

function startRpcClient({ databasePath, environment }) {
  const child = spawn(serverPath, ["--db", databasePath], {
    cwd: repositoryRoot,
    env: environment,
    stdio: ["pipe", "pipe", "pipe"],
  });
  let buffer = "";
  let diagnostics = "";
  let requestId = 0;
  const pending = new Map();
  const events = [];
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    buffer += chunk;
    let newline = buffer.indexOf("\n");
    while (newline >= 0) {
      const line = buffer.slice(0, newline).trim();
      buffer = buffer.slice(newline + 1);
      newline = buffer.indexOf("\n");
      if (!line) continue;
      const message = JSON.parse(line);
      if (message.method === "session.event") {
        events.push(message.params);
        continue;
      }
      if (message.id == null) continue;
      const request = pending.get(message.id);
      if (!request) continue;
      pending.delete(message.id);
      if (message.error) request.reject(new Error(`RPC ${message.error.code}: ${message.error.message}`));
      else request.resolve(message.result);
    }
  });
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => {
    diagnostics += chunk;
  });
  const rejectAll = (error) => {
    for (const request of pending.values()) request.reject(error);
    pending.clear();
  };
  child.once("error", rejectAll);
  child.once("exit", (code) => {
    if (pending.size) rejectAll(new Error(`xcoding-server exited with ${code}: ${diagnostics.trim()}`));
  });

  return {
    events,
    request(method, params) {
      const id = ++requestId;
      const response = new Promise((resolveRequest, reject) => pending.set(id, { resolve: resolveRequest, reject }));
      child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
      return response;
    },
    async close() {
      if (child.exitCode !== null) return;
      child.stdin.end();
      await new Promise((resolveExit) => child.once("exit", resolveExit));
    },
  };
}

async function startMockProvider() {
  const state = { scenario: "hard-deny" };
  const server = createServer(async (request, response) => {
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const payload = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    const messages = payload.messages ?? [];
    const toolMessages = messages.filter((message) => message.role === "tool");
    const hasToolResult = toolMessages.length > 0;

    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
    });

    if (state.scenario === "hard-deny") {
      if (hasToolResult) {
        writeText(response, "Hard-denied command failed as expected.");
        return;
      }
      writeToolCall(response, {
        id: "call-hard-deny",
        name: "run_command",
        arguments: {
          executable: "format",
          args: ["C:"],
        },
      });
      return;
    }

    if (state.scenario === "workspace-deny") {
      if (hasToolResult) {
        writeText(response, "Workspace denylist blocked cargo --version.");
        return;
      }
      writeToolCall(response, {
        id: "call-workspace-deny",
        name: "run_command",
        arguments: {
          executable: "cargo",
          args: ["--version"],
        },
      });
      return;
    }

    writeText(response, "Unexpected scenario.");
  });

  await new Promise((resolveListen) => server.listen(0, "127.0.0.1", resolveListen));
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  return {
    port,
    get baseUrl() {
      return `http://127.0.0.1:${port}/v1`;
    },
    set scenario(value) {
      state.scenario = value;
    },
    close() {
      return new Promise((resolveClose, rejectClose) => {
        server.close((error) => (error ? rejectClose(error) : resolveClose()));
      });
    },
  };
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
