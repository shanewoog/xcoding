import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { createServer } from "node:http";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const binaryName = process.platform === "win32" ? "xcoding-server.exe" : "xcoding-server";
const serverPath = resolve(repositoryRoot, "target/debug", binaryName);

async function main() {
  const fixtureRoot = await mkdtemp(resolve(tmpdir(), "xcoding-git-fixture-"));
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-git-"));
  await prepareGitFixture(fixtureRoot);
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
    const result = await rpc.request("session.chat", {
      workspace_root: fixtureRoot,
      message: "What local git changes are present?",
      model: "fixture-model",
    });

    assert.equal(result.session.status, "done");
    assert.match(result.message.content, /hello\.txt|modified/i);

    assert.ok(
      rpc.events.some((event) => event.type === "tool_start" && event.tool_call?.name === "git_status"),
      "expected git_status tool start",
    );
    assert.ok(
      rpc.events.some(
        (event) =>
          event.type === "tool_end" &&
          event.tool_call?.name === "git_status" &&
          event.success === true,
      ),
      "expected git_status tool end",
    );
    assert.ok(
      rpc.events.some((event) => event.type === "tool_start" && event.tool_call?.name === "git_diff"),
      "expected git_diff tool start",
    );

    assert.equal(mock.requests.length, 3);
    assert.deepEqual(
      mock.requests[0].tools.map((tool) => tool.function.name),
      ["list_dir", "read_file", "search_code", "apply_patch", "run_command", "git_status", "git_diff"],
    );

    const statusTool = mock.requests[1].messages.find(
      (message) => message.role === "tool" && message.tool_call_id === "call_git_status",
    );
    assert.ok(statusTool, "status tool result refeeded");
    assert.match(statusTool.content ?? "", /hello\.txt/);

    const diffTool = mock.requests[2].messages.find(
      (message) => message.role === "tool" && message.tool_call_id === "call_git_diff",
    );
    assert.ok(diffTool, "diff tool result refeeded");
    assert.match(diffTool.content ?? "", /hello world/);


    const taskCompleted = rpc.events.find((event) => event.type === "task_completed");
    assert.ok(taskCompleted, "expected task_completed event");
    assert.ok(taskCompleted.summary?.git_branch, "expected git_branch in task summary");
    assert.ok(taskCompleted.summary?.git_status, "expected git_status in task summary");
    assert.match(taskCompleted.summary.git_status, /hello\.txt/);
    assert.ok(taskCompleted.summary?.git_diff, "expected git_diff in task summary");
    assert.match(taskCompleted.summary.git_diff, /hello world/);
    console.log("Git tools agent E2E passed.");
  } finally {
    await rpc.close();
    await mock.close();
    await rm(databaseDirectory, { recursive: true, force: true });
    await rm(fixtureRoot, { recursive: true, force: true });
  }
}

async function prepareGitFixture(root) {
  execFileSync("git", ["init"], { cwd: root, stdio: "ignore" });
  execFileSync("git", ["config", "user.email", "xcoding@example.com"], { cwd: root, stdio: "ignore" });
  execFileSync("git", ["config", "user.name", "XCoding"], { cwd: root, stdio: "ignore" });
  await writeFile(resolve(root, "hello.txt"), "hello\n", "utf8");
  execFileSync("git", ["add", "hello.txt"], { cwd: root, stdio: "ignore" });
  execFileSync("git", ["commit", "-m", "init"], { cwd: root, stdio: "ignore" });
  await writeFile(resolve(root, "hello.txt"), "hello world\n", "utf8");
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
    if (turn === 0) {
      response.write(
        `data: ${JSON.stringify({
          choices: [
            {
              delta: {
                tool_calls: [
                  {
                    index: 0,
                    id: "call_git_status",
                    type: "function",
                    function: {
                      name: "git_status",
                      arguments: JSON.stringify({}),
                    },
                  },
                ],
              },
            },
          ],
        })}\n\n`,
      );
    } else if (turn === 1) {
      response.write(
        `data: ${JSON.stringify({
          choices: [
            {
              delta: {
                tool_calls: [
                  {
                    index: 0,
                    id: "call_git_diff",
                    type: "function",
                    function: {
                      name: "git_diff",
                      arguments: JSON.stringify({ path: "hello.txt" }),
                    },
                  },
                ],
              },
            },
          ],
        })}\n\n`,
      );
    } else {
      response.write(
        `data: ${JSON.stringify({
          choices: [{ delta: { content: "hello.txt is modified in the working tree." } }],
        })}\n\n`,
      );
    }
    turn += 1;
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
