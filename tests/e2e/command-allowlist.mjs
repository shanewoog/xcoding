import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { cp, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const fixtureSource = resolve(repositoryRoot, "tests/e2e/fixtures/read-only-agent");
const binaryName = process.platform === "win32" ? "xcoding-server.exe" : "xcoding-server";
const serverPath = resolve(repositoryRoot, "target/debug", binaryName);

async function main() {
  const workspace = await mkdtemp(resolve(tmpdir(), "xcoding-cmd-allow-"));
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-cmd-allow-db-"));
  await cp(fixtureSource, workspace, { recursive: true });
  const mock = await startMockProvider();
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "e2e-test-key",
      XCODING_OPENAI_BASE_URL: mock.baseUrl,
    },
  });

  try {
    mock.scenario = "ask-allowlisted";
    const ask = await rpc.request("session.chat", {
      workspace_root: workspace,
      message: "Run cargo version.",
      model: "fixture-model",
      mode: "ask",
    });
    assert.equal(ask.session.status, "need_user");
    const askApproval = eventFor(rpc, ask.session.id, "approval_requested");
    assert.equal(askApproval.action.tool_call.name, "run_command");
    assert.equal(askApproval.action.tool_call.arguments.executable, "cargo");
    const askStart = eventFor(rpc, ask.session.id, "tool_start");
    assert.match(String(askStart.summary), /Awaiting approval for run_command/i);
    const afterAskReject = await rpc.request("session.resolve", {
      session_id: ask.session.id,
      action_id: askApproval.action.id,
      approved: false,
    });
    assert.equal(afterAskReject.session.status, "done");

    mock.scenario = "auto-allowlisted";
    const auto = await rpc.request("session.chat", {
      workspace_root: workspace,
      message: "Run cargo version automatically.",
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
    const autoEnd = eventsFor(rpc, auto.session.id).find(
      (event) => event.type === "tool_end" && event.tool_call?.name === "run_command",
    );
    assert.ok(autoEnd);
    assert.equal(autoEnd.success, true);
    assert.match(String(auto.message?.content ?? ""), /allowlisted command completed/i);

    mock.scenario = "auto-high-risk";
    const gated = await rpc.request("session.chat", {
      workspace_root: workspace,
      message: "Run a shell echo.",
      model: "fixture-model",
      mode: "auto-edit",
    });
    assert.equal(gated.session.status, "need_user");
    const riskApproval = eventFor(rpc, gated.session.id, "approval_requested");
    assert.equal(riskApproval.action.tool_call.name, "run_command");
    assert.match(String(riskApproval.summary), /HIGH-RISK|approve command/i);
    const riskStart = eventsFor(rpc, gated.session.id).find(
      (event) => event.type === "tool_start" && event.tool_call?.name === "run_command",
    );
    assert.ok(riskStart);
    assert.match(String(riskStart.summary), /Awaiting approval for run_command/i);
    const afterRiskReject = await rpc.request("session.resolve", {
      session_id: gated.session.id,
      action_id: riskApproval.action.id,
      approved: false,
    });
    assert.equal(afterRiskReject.session.status, "done");

    console.log("Command allowlist policy checks passed.");
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
  const state = { scenario: "ask-allowlisted" };
  const server = createServer(async (request, response) => {
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const payload = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    const messages = payload.messages ?? [];
    const toolMessages = messages.filter((message) => message.role === "tool");
    const hasToolResult = toolMessages.length > 0;
    const hasRejection = toolMessages.some((message) =>
      String(message.content ?? "").includes('"rejected":true'),
    );

    response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });

    if (state.scenario === "ask-allowlisted" || state.scenario === "auto-allowlisted") {
      if (hasToolResult || hasRejection) {
        writeText(
          response,
          state.scenario === "auto-allowlisted"
            ? "The allowlisted command completed without approval."
            : "The allowlisted command stayed gated under ask mode.",
        );
      } else {
        writeTool(response, "call_cargo", "run_command", {
          executable: "cargo",
          args: ["--version"],
        });
      }
      response.end("data: [DONE]\n\n");
      return;
    }

    if (state.scenario === "auto-high-risk") {
      if (hasToolResult || hasRejection) {
        writeText(response, "The high-risk command remained gated.");
      } else {
        const executable = process.platform === "win32" ? "cmd" : "bash";
        const args =
          process.platform === "win32" ? ["/c", "echo", "risk"] : ["-lc", "echo risk"];
        writeTool(response, "call_shell", "run_command", { executable, args });
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