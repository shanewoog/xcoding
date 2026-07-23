import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const binaryName = process.platform === "win32" ? "xcoding-server.exe" : "xcoding-server";
const serverPath = resolve(repositoryRoot, "target/debug", binaryName);

async function main() {
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-provider-status-"));
  const missing = startRpcClient({
    databasePath: resolve(databaseDirectory, "missing.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "",
      XCODING_OPENAI_BASE_URL: "https://example.test/v1",
    },
  });
  const present = startRpcClient({
    databasePath: resolve(databaseDirectory, "present.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "sk-test-key-abcdef",
      XCODING_OPENAI_BASE_URL: "https://example.test/v1/",
    },
  });

  try {
    const missingStatus = await missing.request("provider.status", {});
    assert.equal(missingStatus.has_api_key, false);
    assert.equal(missingStatus.ready, false);
    assert.equal(missingStatus.base_url, "https://example.test/v1");
    assert.match(missingStatus.message, /OPENAI_API_KEY is not set/);

    const presentStatus = await present.request("provider.status", {});
    assert.equal(presentStatus.has_api_key, true);
    assert.equal(presentStatus.ready, true);
    assert.equal(presentStatus.base_url, "https://example.test/v1");
    assert.equal(presentStatus.key_hint, "...cdef");
    assert.match(presentStatus.message, /OPENAI_API_KEY is set/);

    console.log("Provider status E2E passed.");
  } finally {
    await missing.close();
    await present.close();
    await rm(databaseDirectory, { recursive: true, force: true });
  }
}

function startRpcClient({ databasePath, environment }) {
  // Keep OPENAI_API_KEY as "" when testing the missing-key path so dotenvy cannot
  // refill it from a repo-root .env (dotenvy does not override existing vars).
  const env = { ...environment };
  if (env.OPENAI_API_KEY == null) {
    env.OPENAI_API_KEY = "";
  }

  const child = spawn(serverPath, ["--db", databasePath], {
    cwd: repositoryRoot,
    env,
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
      if (message.id != null && pending.has(message.id)) {
        const { resolve, reject } = pending.get(message.id);
        pending.delete(message.id);
        if (message.error) {
          reject(new Error(`RPC ${message.error.code}: ${message.error.message}`));
        } else {
          resolve(message.result);
        }
      }
    }
  });
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => {
    diagnostics += chunk;
  });

  return {
    request(method, params) {
      const id = ++requestId;
      const payload = JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n";
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        child.stdin.write(payload, (error) => {
          if (error) reject(error);
        });
        setTimeout(() => {
          if (pending.has(id)) {
            pending.delete(id);
            reject(new Error(`timeout waiting for ${method}: ${diagnostics}`));
          }
        }, 10_000);
      });
    },
    async close() {
      child.kill();
      await new Promise((resolve) => child.once("exit", resolve));
    },
  };
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
