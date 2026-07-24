import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const cliPath = resolve(repositoryRoot, "apps/cli/dist/index.js");
const binaryName = process.platform === "win32" ? "xcoding-server.exe" : "xcoding-server";
const serverPath = resolve(repositoryRoot, "target/debug", binaryName);

function runDoctor(workspace, env = {}) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(
      process.execPath,
      [cliPath, "doctor", "--workspace", workspace, "--server", serverPath],
      {
        cwd: repositoryRoot,
        env: { ...process.env, ...env },
        windowsHide: true,
      },
    );
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    child.on("error", reject);
    child.on("close", (code) => {
      resolvePromise({ code, stdout, stderr });
    });
  });
}

async function main() {
  const workspace = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-doctor-"));
  try {
    const missing = await runDoctor(workspace, {
      OPENAI_API_KEY: "",
      XCODING_OPENAI_BASE_URL: "https://example.test/v1",
    });
    const missingReport = JSON.parse(missing.stdout);
    assert.equal(missingReport.ready, false);
    assert.equal(missing.code, 2);
    const auth = missingReport.checks.find((check) => check.name === "provider_auth");
    assert.ok(auth);
    assert.equal(auth.ok, false);

    const present = await runDoctor(workspace, {
      OPENAI_API_KEY: "sk-test-key-abcdef",
      XCODING_OPENAI_BASE_URL: "https://example.test/v1",
    });
    const presentReport = JSON.parse(present.stdout);
    assert.equal(presentReport.ready, true);
    assert.equal(present.code, 0);
    for (const name of [
      "workspace",
      "server_binary",
      "core_rpc",
      "provider_auth",
      "workspace_config",
      "git",
      "mcp_config",
    ]) {
      const check = presentReport.checks.find((item) => item.name === name);
      assert.ok(check, `missing check ${name}`);
      assert.equal(check.ok, true, `${name} should pass: ${check.detail}`);
    }

    console.log("Doctor E2E passed.");
  } finally {
    await rm(workspace, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
