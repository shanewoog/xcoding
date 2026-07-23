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
  const fixtureRoot = await mkdtemp(resolve(tmpdir(), "xcoding-git-fetch-pull-fixture-"));
  const bareRoot = await mkdtemp(resolve(tmpdir(), "xcoding-git-fetch-pull-remote-"));
  const peerRoot = await mkdtemp(resolve(tmpdir(), "xcoding-git-fetch-pull-peer-"));
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-git-fetch-pull-"));
  await prepareGitFixture(fixtureRoot, bareRoot, peerRoot);
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
    // ask mode: git_fetch always requires approval, then git_pull
    const started = await rpc.request("session.chat", {
      workspace_root: fixtureRoot,
      message: "Fetch and pull main from origin",
      model: "fixture-model",
      mode: "ask",
    });
    assert.equal(started.session.status, "need_user");
    const fetchApproval = latestApproval(rpc, started.session.id);
    assert.equal(fetchApproval.action.tool_call.name, "git_fetch");
    assert.match(fetchApproval.summary ?? "", /git fetch|HIGH-RISK/i);

    const afterFetch = await rpc.request("session.resolve", {
      session_id: started.session.id,
      action_id: fetchApproval.action.id,
      approved: true,
    });
    assert.equal(afterFetch.session.status, "need_user");
    const pullApproval = latestApproval(rpc, started.session.id);
    assert.equal(pullApproval.action.tool_call.name, "git_pull");
    assert.match(pullApproval.summary ?? "", /git pull|HIGH-RISK/i);

    const completed = await rpc.request("session.resolve", {
      session_id: started.session.id,
      action_id: pullApproval.action.id,
      approved: true,
    });
    assert.equal(completed.session.status, "done");
    assert.match(completed.message?.content ?? "", /pull|fetch/i);

    assert.ok(
      rpc.events.some(
        (event) =>
          event.type === "tool_end" &&
          event.tool_call?.name === "git_fetch" &&
          event.success === true,
      ),
      "expected successful git_fetch tool_end",
    );
    assert.ok(
      rpc.events.some(
        (event) =>
          event.type === "tool_end" &&
          event.tool_call?.name === "git_pull" &&
          event.success === true,
      ),
      "expected successful git_pull tool_end",
    );

    const localHead = execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: fixtureRoot,
      encoding: "utf8",
    }).trim();
    const peerHead = execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: peerRoot,
      encoding: "utf8",
    }).trim();
    assert.equal(localHead, peerHead);
    assert.equal(
      execFileSync("git", ["show", "HEAD:hello.txt"], {
        cwd: fixtureRoot,
        encoding: "utf8",
      }),
      "hello from peer\n",
    );

    // Advance remote again for auto-edit mode
    await writeFile(resolve(peerRoot, "hello.txt"), "hello auto pull\n", "utf8");
    execFileSync("git", ["add", "hello.txt"], { cwd: peerRoot, stdio: "ignore" });
    execFileSync("git", ["commit", "-m", "auto peer advance"], {
      cwd: peerRoot,
      stdio: "ignore",
    });
    execFileSync("git", ["push", "origin", "main"], { cwd: peerRoot, stdio: "ignore" });
    const peerHead2 = execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: peerRoot,
      encoding: "utf8",
    }).trim();

    await rpc.request("config.set", {
      workspace_root: fixtureRoot,
      mode: "auto-edit",
      provider: "openai",
      model: "fixture-model",
    });

    const autoStarted = await rpc.request("session.chat", {
      workspace_root: fixtureRoot,
      message: "Fetch and pull again",
      model: "fixture-model",
    });
    assert.equal(autoStarted.session.mode, "auto-edit");
    assert.equal(autoStarted.session.status, "need_user");
    const autoFetch = latestApproval(rpc, autoStarted.session.id);
    assert.equal(autoFetch.action.tool_call.name, "git_fetch");

    const autoAfterFetch = await rpc.request("session.resolve", {
      session_id: autoStarted.session.id,
      action_id: autoFetch.action.id,
      approved: true,
    });
    assert.equal(autoAfterFetch.session.status, "need_user");
    const autoPull = latestApproval(rpc, autoStarted.session.id);
    assert.equal(autoPull.action.tool_call.name, "git_pull");

    const autoDone = await rpc.request("session.resolve", {
      session_id: autoStarted.session.id,
      action_id: autoPull.action.id,
      approved: true,
    });
    assert.equal(autoDone.session.status, "done");

    const localHead2 = execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: fixtureRoot,
      encoding: "utf8",
    }).trim();
    assert.equal(localHead2, peerHead2);

    const tools = mock.requests.flatMap((payload) =>
      (payload.tools ?? []).map((tool) => tool.function?.name ?? tool.name),
    );
    for (const name of [
      "list_dir",
      "read_file",
      "search_code",
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
    ]) {
      assert.ok(tools.includes(name), `expected tool definition ${name}`);
    }

    console.log("Git fetch/pull agent E2E passed.");
  } finally {
    await rpc.close();
    await mock.close();
    await rm(databaseDirectory, { recursive: true, force: true });
    await rm(fixtureRoot, { recursive: true, force: true });
    await rm(peerRoot, { recursive: true, force: true });
    await rm(bareRoot, { recursive: true, force: true });
  }
}

function latestApproval(rpc, sessionId) {
  const approvals = rpc.events.filter(
    (event) => event.type === "approval_requested" && event.session_id === sessionId,
  );
  assert.ok(approvals.length > 0, `session ${sessionId} should request approval`);
  return approvals[approvals.length - 1];
}

async function prepareGitFixture(root, bareRoot, peerRoot) {
  execFileSync("git", ["init", "-b", "main"], { cwd: root, stdio: "ignore" });
  execFileSync("git", ["init", "--bare", "-b", "main", bareRoot], { stdio: "ignore" });
  execFileSync("git", ["config", "user.email", "xcoding@example.com"], {
    cwd: root,
    stdio: "ignore",
  });
  execFileSync("git", ["config", "user.name", "XCoding"], { cwd: root, stdio: "ignore" });
  await writeFile(resolve(root, "hello.txt"), "hello\n", "utf8");
  execFileSync("git", ["add", "hello.txt"], { cwd: root, stdio: "ignore" });
  execFileSync("git", ["commit", "-m", "init"], { cwd: root, stdio: "ignore" });
  execFileSync("git", ["remote", "add", "origin", bareRoot], { cwd: root, stdio: "ignore" });
  execFileSync("git", ["push", "--set-upstream", "origin", "main"], {
    cwd: root,
    stdio: "ignore",
  });

  execFileSync("git", ["clone", bareRoot, peerRoot], { stdio: "ignore" });
  execFileSync("git", ["config", "user.email", "xcoding@example.com"], {
    cwd: peerRoot,
    stdio: "ignore",
  });
  execFileSync("git", ["config", "user.name", "XCoding"], { cwd: peerRoot, stdio: "ignore" });
  await writeFile(resolve(peerRoot, "hello.txt"), "hello from peer\n", "utf8");
  execFileSync("git", ["add", "hello.txt"], { cwd: peerRoot, stdio: "ignore" });
  execFileSync("git", ["commit", "-m", "peer advance"], { cwd: peerRoot, stdio: "ignore" });
  execFileSync("git", ["push", "origin", "main"], { cwd: peerRoot, stdio: "ignore" });
}

function startRpcClient({ databasePath, environment }) {
  const child = spawn(serverPath, ["--db", databasePath], {
    cwd: repositoryRoot,
    env: {
      ...environment,
    },
    stdio: ["pipe", "pipe", "pipe"],
  });
  let requestId = 0;
  const pending = new Map();
  const events = [];
  let outputBuffer = "";
  let diagnostics = "";
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
      if (message.method === "session.event") {
        events.push(message.params);
        continue;
      }
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
  const rejectAll = (error) => {
    for (const request of pending.values()) request.reject(error);
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
      const response = new Promise((resolveRequest, reject) =>
        pending.set(id, { resolve: resolveRequest, reject }),
      );
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
  const requests = [];
  const server = createServer(async (request, response) => {
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const payload = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    requests.push(payload);
    const messages = payload.messages ?? [];
    const lastTool = [...messages].reverse().find((message) => message.role === "tool");

    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
    });

    if (!lastTool) {
      response.write(
        `data: ${JSON.stringify({
          choices: [
            {
              delta: {
                tool_calls: [
                  {
                    index: 0,
                    id: "call_git_fetch",
                    type: "function",
                    function: {
                      name: "git_fetch",
                      arguments: JSON.stringify({
                        remote: "origin",
                        branch: "main",
                      }),
                    },
                  },
                ],
              },
            },
          ],
        })}\n\n`,
      );
    } else if (lastTool.tool_call_id === "call_git_fetch") {
      response.write(
        `data: ${JSON.stringify({
          choices: [
            {
              delta: {
                tool_calls: [
                  {
                    index: 0,
                    id: "call_git_pull",
                    type: "function",
                    function: {
                      name: "git_pull",
                      arguments: JSON.stringify({
                        remote: "origin",
                        branch: "main",
                        ff_only: true,
                      }),
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
          choices: [
            {
              delta: {
                content: "Fetched and pulled main from origin with approved git tools.",
              },
            },
          ],
        })}\n\n`,
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
