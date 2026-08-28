import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const fixtureSource = resolve(repositoryRoot, "tests/e2e/fixtures/read-only-agent");
const binaryName = process.platform === "win32" ? "xcoding-server.exe" : "xcoding-server";
const serverPath = resolve(repositoryRoot, "target/debug", binaryName);

async function main() {
  const workspace = await mkdtemp(resolve(tmpdir(), "xcoding-full-auto-"));
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-full-auto-db-"));
  const homeDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-full-auto-home-"));
  const configDirectory = resolve(homeDirectory, ".xcoding");
  await cp(fixtureSource, workspace, { recursive: true });
  const mock = await startMockProvider();
  await mkdir(configDirectory, { recursive: true });
  await writeFile(
    resolve(configDirectory, "config.json"),
    `${JSON.stringify(
      {
        provider_fallback_enabled: false,
        providers: [
          { id: "default", name: "openai", base_url: mock.baseUrl, api_key: "e2e-test-key", trust_level: "official" },
        ],
        active_provider_id: "default",
      },
      null,
    )}\n`,
    "utf8",
  );
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      // Isolate from the developer's ~/.xcoding/config.json provider credentials.
      USERPROFILE: homeDirectory,
      HOME: homeDirectory,
      OPENAI_API_KEY: "e2e-test-key",
      XCODING_OPENAI_BASE_URL: mock.baseUrl,
    },
  });

  try {
    // Full auto auto-applies writes to high-risk paths without approval.
    mock.scenario = "full-auto-high-risk-path";
    const highRisk = await rpc.request("session.chat", {
      workspace_root: workspace,
      message: "Touch protected path.",
      model: "fixture-model",
      mode: "full-auto",
    });
    assert.equal(highRisk.session.mode, "full-auto");
    assert.equal(highRisk.session.status, "done");
    assert.equal(
      await readFile(resolve(workspace, ".xcoding/secret.txt"), "utf8"),
      "trusted\n",
    );
    assert.equal(
      eventsFor(rpc, highRisk.session.id).some((event) => event.type === "approval_requested"),
      false,
    );
    const highRiskStart = eventsFor(rpc, highRisk.session.id).find(
      (event) => event.type === "tool_start" && event.tool_call?.name === "apply_patch",
    );
    assert.ok(highRiskStart);
    assert.match(String(highRiskStart.summary), /Auto-applying apply_patch/i);

    // Full auto auto-runs commands that auto-edit would gate behind approval.
    mock.scenario = "full-auto-command";
    const command = await rpc.request("session.chat", {
      workspace_root: workspace,
      message: "Run a command.",
      model: "fixture-model",
      mode: "full-auto",
    });
    assert.equal(command.session.status, "done");
    assert.equal(
      eventsFor(rpc, command.session.id).some((event) => event.type === "approval_requested"),
      false,
    );
    const commandStart = eventsFor(rpc, command.session.id).find(
      (event) => event.type === "tool_start" && event.tool_call?.name === "run_command",
    );
    assert.ok(commandStart);
    assert.match(String(commandStart.summary), /Auto-running run_command/i);
    assert.ok(
      eventsFor(rpc, command.session.id).some(
        (event) =>
          event.type === "tool_end" && event.tool_call?.name === "run_command" && event.success === true,
      ),
    );

    console.log("Full auto mode policy checks passed.");
  } finally {
    await rpc.close();
    await mock.close();
    await rm(workspace, { recursive: true, force: true });
    await rm(databaseDirectory, { recursive: true, force: true });
    await rm(homeDirectory, { recursive: true, force: true });
  }
}

function eventsFor(rpc, sessionId) {
  return rpc.events.filter((event) => event.session_id === sessionId);
}

function eventFor(rpc, sessionId, type) {
  const event = eventsFor(rpc, sessionId).find((item) => item.type === type);
  assert.ok(event, `expected ${type} for ${sessionId}`);
  return event;
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
  const state = { scenario: "full-auto-high-risk-path" };
  const server = createServer(async (request, response) => {
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const payload = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    const messages = payload.messages ?? [];
    const hasToolResult = messages.some((message) => message.role === "tool");

    response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });

    if (state.scenario === "full-auto-high-risk-path") {
      if (hasToolResult) {
        writeText(response, "Protected path was written under full auto.");
      } else {
        writeTool(response, "call_secret", "apply_patch", {
          path: ".xcoding/secret.txt",
          old_text: "",
          new_text: "trusted\n",
        });
      }
      response.end("data: [DONE]\n\n");
      return;
    }

    if (state.scenario === "full-auto-command") {
      if (hasToolResult) {
        writeText(response, "Command ran without approval.");
      } else {
        writeTool(response, "call_cargo", "run_command", {
          executable: "cargo",
          args: ["--version"],
        });
      }
      response.end("data: [DONE]\n\n");
      return;
    }

    writeText(response, "Unhandled scenario.");
    response.end("data: [DONE]\n\n");
  });
  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  assert.ok(address && typeof address !== "string");
  return {
    get scenario() {
      return state.scenario;
    },
    set scenario(value) {
      state.scenario = value;
    },
    baseUrl: `http://127.0.0.1:${address.port}/v1`,
    close: () =>
      new Promise((resolveClose, rejectClose) =>
        server.close((error) => (error ? rejectClose(error) : resolveClose())),
      ),
  };
}

function writeText(response, content) {
  response.write(`data: ${JSON.stringify({ choices: [{ delta: { content } }] })}\n\n`);
}

function writeTool(response, id, name, args) {
  response.write(
    `data: ${JSON.stringify({
      choices: [
        {
          delta: {
            tool_calls: [
              {
                index: 0,
                id,
                type: "function",
                function: { name, arguments: JSON.stringify(args) },
              },
            ],
          },
        },
      ],
    })}\n\n`,
  );
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : String(error));
  process.exitCode = 1;
});
