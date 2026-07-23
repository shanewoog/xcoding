import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile, mkdir, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const serverPath = resolve(repositoryRoot, "target/debug/xcoding-server.exe");

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
  await import("node:fs").then((fs) => {
    if (!fs.existsSync(serverPath)) {
      throw new Error("build xcoding-server first: cargo build -p xcoding-server");
    }
  });

  const workspace = await mkdtemp(join(tmpdir(), "xcoding-custom-allowlist-"));
  const databaseDirectory = await mkdtemp(join(tmpdir(), "xcoding-custom-allowlist-db-"));
  const databasePath = join(databaseDirectory, "xcoding.db");
  await writeFile(join(workspace, "README.md"), "# custom allowlist fixture\n", "utf8");

  const mock = await startMockProvider();
  const environment = {
    ...process.env,
    OPENAI_API_KEY: "test-key",
    XCODING_OPENAI_BASE_URL: `http://127.0.0.1:${mock.port}/v1`,
  };
  const rpc = startRpcClient({ databasePath, environment });

  try {
    // Persist custom allowlist through config.set
    const saved = await rpc.request("config.set", {
      workspace_root: workspace,
      mode: "auto-edit",
      provider: "openai",
      model: "fixture-model",
      command_allowlist: ["git:--version", "rg"],
    });
    assert.deepEqual(saved.config.command_allowlist, ["git:--version", "rg"]);
    const fileBody = await readFile(join(workspace, ".xcoding", "command-allowlist"), "utf8");
    assert.match(fileBody, /git:--version/);
    assert.match(fileBody, /^rg$/m);

    const loaded = await rpc.request("config.get", { workspace_root: workspace });
    assert.deepEqual(loaded.config.command_allowlist, ["git:--version", "rg"]);

    // Reject never-allowlisted shells
    await assert.rejects(
      () =>
        rpc.request("config.set", {
          workspace_root: workspace,
          mode: "auto-edit",
          provider: "openai",
          model: "fixture-model",
          command_allowlist: ["powershell"],
        }),
      /cannot be added|allowlist|Invalid|invalid/i,
    );

    mock.scenario = "auto-custom";
    const auto = await rpc.request("session.chat", {
      workspace_root: workspace,
      message: "Run git --version with custom allowlist.",
      model: "fixture-model",
      mode: "auto-edit",
    });
    assert.equal(auto.session.status, "done");
    assert.equal(
      eventsFor(rpc, auto.session.id).some((event) => event.type === "approval_requested"),
      false,
    );
    const autoStart = eventsFor(rpc, auto.session.id).find(
      (event) => event.type === "tool_start" && event.tool_call?.name === "run_command",
    );
    assert.ok(autoStart);
    assert.match(String(autoStart.summary), /Auto-running run_command/i);
    assert.match(String(auto.message?.content ?? ""), /custom allowlisted command completed/i);

    console.log("Custom command allowlist checks passed.");
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
  const state = { scenario: "auto-custom" };
  const server = createServer(async (request, response) => {
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const payload = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    const messages = payload.messages ?? [];
    const toolMessages = messages.filter((message) => message.role === "tool");
    const hasToolResult = toolMessages.length > 0;

    response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });

    if (hasToolResult) {
      writeText(response, "The custom allowlisted command completed without approval.");
      return;
    }

    writeToolCall(response, {
      id: "tool-custom-git-version",
      name: "run_command",
      arguments: {
        executable: "git",
        args: ["--version"],
      },
    });
  });

  await new Promise((resolveListen) => server.listen(0, "127.0.0.1", resolveListen));
  const address = server.address();
  return {
    port: address.port,
    get scenario() {
      return state.scenario;
    },
    set scenario(value) {
      state.scenario = value;
    },
    close() {
      return new Promise((resolveClose, reject) => {
        server.close((error) => (error ? reject(error) : resolveClose()));
      });
    },
  };
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
