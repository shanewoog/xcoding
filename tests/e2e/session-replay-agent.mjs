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
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-replay-"));
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
    const chat = await rpc.request("session.chat", {
      workspace_root: workspace,
      message: "Explain auth module.",
      model: "fixture-model",
      mode: "ask",
    });
    assert.equal(chat.session.status, "done");
    assert.ok(chat.message?.content.includes("auth"));

    const replay = await rpc.request("session.replay", {
      session_id: chat.session.id,
    });
    assert.equal(replay.session.id, chat.session.id);
    assert.ok(Array.isArray(replay.events) && replay.events.length > 0);
    assert.ok(Array.isArray(replay.steps) && replay.steps.length > 0);

    const kinds = replay.steps.map((step) => step.kind);
    assert.ok(kinds.includes("tool_start"), `missing tool_start in ${kinds.join(",")}`);
    assert.ok(kinds.includes("tool_end"), `missing tool_end in ${kinds.join(",")}`);
    assert.ok(kinds.includes("assistant_message"), `missing assistant_message in ${kinds.join(",")}`);

    const toolStart = replay.steps.find((step) => step.kind === "tool_start");
    assert.equal(toolStart?.tool_name, "list_dir");
    const toolEnd = replay.steps.find((step) => step.kind === "tool_end");
    assert.equal(toolEnd?.success, true);
    assert.match(toolEnd?.summary ?? "", /./);

    console.log("Session replay E2E passed.");
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
  child.once("error", (error) => rejectPending(error));
  child.once("exit", (code) => {
    if (pending.size > 0) {
      rejectPending(new Error(`xcoding-server exited with ${code}: ${diagnostics.trim()}`));
    }
  });

  function rejectPending(error) {
    for (const { reject } of pending.values()) {
      reject(error);
    }
    pending.clear();
  }

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
  let turn = 0;
  const server = createServer(async (request, response) => {
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    const chunks = [];
    for await (const chunk of request) {
      chunks.push(chunk);
    }
    requests.push(JSON.parse(Buffer.concat(chunks).toString("utf8")));

    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
    });
    if (turn++ === 0) {
      response.write(
        'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_list_root","type":"function","function":{"name":"list_dir","arguments":"{\\"path\\":\\".\\"}"}}]}}]}\n\n',
      );
    } else {
      response.write(
        'data: {"choices":[{"delta":{"content":"The repository contains src/auth.ts."}}]}\n\n',
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
    requests,
    close: () => new Promise((resolveClose, rejectClose) => server.close((error) => error ? rejectClose(error) : resolveClose())),
  };
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : String(error));
  process.exitCode = 1;
});
