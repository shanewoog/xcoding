import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const fixtureRoot = resolve(repositoryRoot, "tests/e2e/fixtures/mcp-agent");
const binaryName = process.platform === "win32" ? "xcoding-server.exe" : "xcoding-server";
const serverPath = resolve(repositoryRoot, "target/debug", binaryName);

async function main() {
  const mock = await startMockProvider();
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-mcp-"));
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "e2e-test-key",
      XCODING_OPENAI_BASE_URL: mock.baseUrl,
    },
  });

  try {
    const pending = await rpc.request("session.chat", {
      workspace_root: fixtureRoot,
      message: "Call the demo MCP echo tool with text hello.",
      model: "fixture-model",
      mode: "ask",
    });
    assert.equal(pending.session.status, "need_user");

    const approval = rpc.events.find((event) => event.type === "approval_requested");
    assert.ok(approval, "MCP call should request approval");
    assert.equal(approval.action.tool_call.name, "mcp");
    assert.match(approval.summary ?? "", /Review MCP demo\.echo/);

    const toolCall = approval.action.tool_call;
    assert.equal(toolCall.arguments.server, "demo");
    assert.equal(toolCall.arguments.tool, "echo");
    assert.equal(toolCall.arguments.arguments.text, "hello");

    const resolved = await rpc.request("session.resolve", {
      session_id: pending.session.id,
      action_id: approval.action.id,
      approved: true,
    });
    assert.equal(resolved.session.status, "done");
    assert.match(resolved.message?.content ?? "", /DONE/);

    const toolEnd = rpc.events.find(
      (event) => event.type === "tool_end" && event.tool_call?.name === "mcp",
    );
    assert.ok(toolEnd, "tool_end for MCP expected");
    assert.equal(toolEnd.success, true);

    assert.ok(mock.requests.length >= 2);
    const firstTools = mock.requests[0].tools.map((tool) => tool.function.name);
    assert.ok(firstTools.includes("mcp__demo__echo"), `tools missing mcp: ${firstTools.join(",")}`);

    const system = mock.requests[0].messages.find((message) => message.role === "system");
    assert.match(system?.content ?? "", /MCP tools/);
    assert.match(system?.content ?? "", /mcp__demo__echo/);

    const toolResult = mock.requests
      .flatMap((request) => request.messages)
      .find((message) => message.role === "tool");
    assert.ok(toolResult, "provider should receive tool result");
    assert.match(toolResult.content ?? "", /echo:hello/);

    console.log("MCP agent E2E passed.");
  } finally {
    await rpc.close();
    await mock.close();
    await rm(databaseDirectory, { recursive: true, force: true });
  }
}

function startRpcClient({ databasePath, environment }) {
  const child = spawn(serverPath, ["--db", databasePath], {
    cwd: repositoryRoot,
    env: environment,
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: true,
  });
  const events = [];
  let outputBuffer = "";
  let diagnostics = "";
  let requestId = 0;
  const pending = new Map();

  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    outputBuffer += chunk;
    let newlineIndex = outputBuffer.indexOf("\n");
    while (newlineIndex >= 0) {
      const line = outputBuffer.slice(0, newlineIndex).trim();
      outputBuffer = outputBuffer.slice(newlineIndex + 1);
      newlineIndex = outputBuffer.indexOf("\n");
      if (!line) {
        continue;
      }
      const message = JSON.parse(line);
      if (message.method === "session.event") {
        events.push(message.params);
        continue;
      }
      const request = pending.get(message.id);
      if (!request) {
        continue;
      }
      pending.delete(message.id);
      if (message.error) {
        request.reject(new Error(`RPC ${message.error.code}: ${message.error.message}`));
      } else {
        request.resolve(message.result);
      }
    }
  });
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => {
    diagnostics += chunk;
  });
  const rejectAll = (error) => {
    for (const request of pending.values()) {
      request.reject(error);
    }
    pending.clear();
  };
  child.once("error", rejectAll);
  child.once("exit", (code) => {
    if (pending.size) {
      rejectAll(new Error(`xcoding-server exited with ${code}: ${diagnostics.trim()}`));
    }
  });

  return {
    events,
    request(method, params) {
      const id = ++requestId;
      const response = new Promise((resolveRequest, reject) => {
        pending.set(id, { resolve: resolveRequest, reject });
      });
      child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
      return response;
    },
    async close() {
      if (child.exitCode !== null) {
        return;
      }
      child.stdin.end();
      await new Promise((resolveExit) => child.once("exit", resolveExit));
    },
  };
}

async function startMockProvider() {
  const requests = [];
  const server = createServer(async (request, response) => {
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    const chunks = [];
    for await (const chunk of request) {
      chunks.push(chunk);
    }
    const payload = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    requests.push(payload);
    const messages = payload.messages ?? [];
    const hasToolResult = messages.some((message) => message.role === "tool");

    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
    });
    if (hasToolResult) {
      response.write(
        `data: ${JSON.stringify({
          choices: [{ delta: { content: "MCP echo returned hello. DONE" } }],
        })}\n\n`,
      );
    } else {
      response.write(
        `data: ${JSON.stringify({
          choices: [
            {
              delta: {
                tool_calls: [
                  {
                    index: 0,
                    id: "call_mcp_echo",
                    type: "function",
                    function: {
                      name: "mcp__demo__echo",
                      arguments: JSON.stringify({ text: "hello" }),
                    },
                  },
                ],
              },
            },
          ],
        })}\n\n`,
      );
    }
    response.end("data: [DONE]\n\n");
  });

  await new Promise((resolveListen) => server.listen(0, "127.0.0.1", resolveListen));
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  return {
    baseUrl: `http://127.0.0.1:${port}/v1`,
    requests,
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
