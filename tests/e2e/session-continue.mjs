import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdtemp, rm, writeFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const serverPath = resolve(repositoryRoot, "target/debug/xcoding-server.exe");

async function main() {
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-continue-"));
  const workspace = resolve(databaseDirectory, "workspace");
  await mkdir(resolve(workspace, "src"), { recursive: true });
  await writeFile(resolve(workspace, "src/auth.ts"), "export const login = () => true;\n", "utf8");

  const mock = await startMockProvider();
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "test-key",
      XCODING_OPENAI_BASE_URL: mock.baseUrl,
      XCODING_OPENAI_MODEL: "fixture-model",
    },
  });

  try {
    const first = await rpc.request("session.chat", {
      workspace_root: workspace,
      message: "Explain auth module.",
      model: "fixture-model",
      mode: "ask",
    });
    assert.equal(first.session.status, "done");
    assert.ok(first.message?.content.toLowerCase().includes("auth"));

    const second = await rpc.request("session.chat", {
      workspace_root: workspace,
      message: "What file holds login?",
      model: "fixture-model",
      mode: "ask",
      session_id: first.session.id,
    });
    assert.equal(second.session.id, first.session.id, "follow-up must reuse session id");
    assert.equal(second.session.status, "done");
    assert.ok(
      second.message?.content.toLowerCase().includes("auth.ts") ||
        second.message?.content.toLowerCase().includes("login"),
      `unexpected follow-up answer: ${second.message?.content}`,
    );

    const { detail } = await rpc.request("session.detail", { session_id: first.session.id });
    const userMessages = detail.messages.filter((message) => message.role === "user");
    assert.equal(userMessages.length, 2);
    assert.equal(userMessages[0].content, "Explain auth module.");
    assert.equal(userMessages[1].content, "What file holds login?");

    const blocked = await rpc
      .request("session.chat", {
        workspace_root: resolve(databaseDirectory, "other-workspace"),
        message: "wrong workspace",
        session_id: first.session.id,
      })
      .then(
        () => null,
        (error) => error,
      );
    assert.ok(blocked, "workspace mismatch must fail");
    assert.match(String(blocked.message || blocked), /workspace/i);

    console.log("Session continue E2E passed.");
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
      if (!line) continue;
      const message = JSON.parse(line);
      if (message.method === "session.event") continue;
      const request = pending.get(message.id);
      if (!request) continue;
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
  child.once("error", (error) => rejectPending(error));
  child.once("exit", (code) => {
    if (pending.size > 0) {
      rejectPending(new Error(`xcoding-server exited with ${code}: ${diagnostics.trim()}`));
    }
  });

  function rejectPending(error) {
    for (const { reject } of pending.values()) reject(error);
    pending.clear();
  }

  return {
    request(method, params) {
      const id = ++requestId;
      const response = new Promise((resolveRequest, reject) => {
        pending.set(id, { resolve: resolveRequest, reject });
      });
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
  let turn = 0;
  const server = createServer(async (request, response) => {
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    const userText = [...(body.messages || [])]
      .reverse()
      .find((message) => message.role === "user")?.content;
    const userBlob = typeof userText === "string" ? userText : JSON.stringify(userText ?? "");

    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
    });

    // First model call of each agent loop may list_dir; later calls answer.
    const round = turn++;
    if (round % 2 === 0) {
      response.write(
        'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_list_root","type":"function","function":{"name":"list_dir","arguments":"{\\"path\\":\\".\\"}"}}]}}]}\n\n',
      );
    } else if (userBlob.toLowerCase().includes("login") || userBlob.toLowerCase().includes("file")) {
      response.write(
        'data: {"choices":[{"delta":{"content":"login is exported from src/auth.ts."}}]}\n\n',
      );
    } else {
      response.write(
        'data: {"choices":[{"delta":{"content":"The repository contains an auth module."}}]}\n\n',
      );
    }
    response.end("data: [DONE]\n\n");
  });

  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  assert.ok(address && typeof address !== "string");

  return {
    baseUrl: `http://127.0.0.1:${address.port}/v1`,
    close: () =>
      new Promise((resolveClose, rejectClose) =>
        server.close((error) => (error ? rejectClose(error) : resolveClose())),
      ),
  };
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : String(error));
  process.exitCode = 1;
});
