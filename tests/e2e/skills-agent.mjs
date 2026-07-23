import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const fixtureRoot = resolve(repositoryRoot, "tests/e2e/fixtures/skills-agent");
const binaryName = process.platform === "win32" ? "xcoding-server.exe" : "xcoding-server";
const serverPath = resolve(repositoryRoot, "target/debug", binaryName);

const EXPECTED_TOOLS = [
  "list_dir",
  "read_file",
  "search_code",
  "load_skill",
  "apply_patch",
  "run_command",
  "git_status",
  "git_diff",
  "git_log",
  "git_show",
  "git_add",
  "git_commit",
  "git_push",
  "git_fetch",
  "git_pull",
];

async function main() {
  const mock = await startMockProvider();
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-skills-"));
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "e2e-test-key",
      XCODING_OPENAI_BASE_URL: mock.baseUrl,
    },
  });

  try {
    const result = await rpc.request("session.chat", {
      workspace_root: fixtureRoot,
      message: "Use the hello-style skill and summarize this workspace.",
      model: "fixture-model",
    });

    assert.equal(result.session.status, "done");
    assert.match(result.message.content, /DONE/);

    const toolStart = rpc.events.find((event) => event.type === "tool_start");
    assert.deepEqual(toolStart?.tool_call, {
      id: "call_load_skill",
      name: "load_skill",
      arguments: { name: "hello-style" },
    });
    assert.equal(rpc.events.find((event) => event.type === "tool_end")?.success, true);

    assert.equal(mock.requests.length, 2);
    assert.deepEqual(
      mock.requests[0].tools.map((tool) => tool.function.name),
      EXPECTED_TOOLS,
    );

    const system = mock.requests[0].messages.find((message) => message.role === "system");
    assert.match(system?.content ?? "", /Workspace skills/);
    assert.match(system?.content ?? "", /hello-style/);
    assert.match(system?.content ?? "", /Prefer concise Chinese summaries/);
    assert.match(system?.content ?? "", /load_skill/);

    const toolResult = mock.requests[1].messages.find((message) => message.role === "tool");
    assert.equal(toolResult?.tool_call_id, "call_load_skill");
    assert.match(toolResult?.content ?? "", /Always end answers with DONE/);
    assert.match(toolResult?.content ?? "", /hello-style/);

    console.log("Skills agent E2E passed.");
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
        'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_load_skill","type":"function","function":{"name":"load_skill","arguments":"{\\"name\\":\\"hello-style\\"}"}}]}}]}\n\n',
      );
    } else {
      response.write(
        'data: {"choices":[{"delta":{"content":"Workspace has the hello-style skill. DONE"}}]}\n\n',
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
