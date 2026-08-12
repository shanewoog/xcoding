import { spawn } from "node:child_process";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = resolve(scriptDirectory, "..");

const desktopE2eTests = [
  "desktop-review.mjs",
  "desktop-activity.mjs",
  "desktop-layout.mjs",
  "desktop-gitnexus.mjs",
  "desktop-config.mjs",
  "desktop-message-links.mjs",
  "task-summary.mjs",
  "surface-parity.mjs",
];

const cliStaticE2eTests = [
  "desktop-review.mjs",
  "desktop-config.mjs",
  "task-summary.mjs",
  "surface-parity.mjs",
];

const knownRustPackages = new Set([
  "agent",
  "context",
  "core",
  "mcp",
  "policy",
  "protocol",
  "providers",
  "server",
  "store",
  "tools",
]);

function normalizePath(filePath) {
  return filePath.trim().replaceAll("\\", "/").replace(/^\.\//, "");
}

function isTemporaryPath(filePath) {
  const normalized = normalizePath(filePath);
  return /(^|\/)(?:_tmp_|tmp[-_]|\.tmp-)/i.test(normalized);
}

function unique(values) {
  return [...new Set(values)];
}

function createPlan() {
  return {
    full: false,
    reasons: [],
    checks: [],
    e2e: [],
    e2eBuild: false,
  };
}

function addReason(plan, reason) {
  if (!plan.reasons.includes(reason)) plan.reasons.push(reason);
}

function addCheck(plan, command) {
  if (!plan.checks.some((item) => item.label === command.label)) plan.checks.push(command);
}

function addE2e(plan, tests, build = false) {
  plan.e2e = unique([...plan.e2e, ...tests]);
  plan.e2eBuild ||= build;
}

function markFull(plan, reason) {
  plan.full = true;
  addReason(plan, reason);
}

function rustPackageFor(filePath) {
  const match = filePath.match(/^crates\/xcoding-([^/]+)\//);
  return match && knownRustPackages.has(match[1]) ? `xcoding-${match[1]}` : undefined;
}

export function selectPlan(inputFiles) {
  const files = unique(inputFiles.map(normalizePath).filter((filePath) => filePath && !isTemporaryPath(filePath)));
  const plan = createPlan();

  for (const filePath of files) {
    if (filePath === "package.json" || filePath === "pnpm-lock.yaml" || filePath === "Cargo.toml" || filePath === "Cargo.lock") {
      markFull(plan, `${filePath} changes dependency or workspace resolution`);
      continue;
    }

    if (filePath.startsWith("tests/e2e/")) {
      const testName = basename(filePath);
      if (testName === "run.mjs") {
        markFull(plan, "the shared e2e runner changed");
      } else if (testName.endsWith(".mjs")) {
        addE2e(plan, [testName]);
        addReason(plan, `${testName} changed`);
      }
      continue;
    }

    if (filePath.startsWith("tests/")) {
      addReason(plan, "test infrastructure changed");
      markFull(plan, "a non-e2e test or test fixture changed");
      continue;
    }

    if (filePath.startsWith("apps/desktop/src/")) {
      addCheck(plan, { label: "desktop TypeScript check", kind: "pnpm", args: ["--filter", "@xcoding/desktop", "check"] });
      addE2e(plan, desktopE2eTests);
      addReason(plan, "desktop source changed");
      continue;
    }

    if (filePath.startsWith("apps/desktop/src-tauri/")) {
      addCheck(plan, {
        label: "Tauri Rust check",
        kind: "cargo",
        args: ["check"],
        cwd: "apps/desktop/src-tauri",
      });
      addE2e(plan, desktopE2eTests);
      addReason(plan, "Tauri desktop source changed");
      continue;
    }

    if (filePath.startsWith("apps/cli/src/")) {
      addCheck(plan, { label: "CLI TypeScript check", kind: "pnpm", args: ["--filter", "@xcoding/cli", "check"] });
      addE2e(plan, cliStaticE2eTests, true);
      addReason(plan, "CLI source changed");
      continue;
    }

    if (filePath.startsWith("packages/protocol/src/")) {
      addCheck(plan, { label: "protocol TypeScript build", kind: "pnpm", args: ["--filter", "@xcoding/protocol", "build"] });
      addCheck(plan, { label: "client TypeScript build", kind: "pnpm", args: ["--filter", "@xcoding/client", "build"] });
      addCheck(plan, { label: "CLI TypeScript check", kind: "pnpm", args: ["--filter", "@xcoding/cli", "check"] });
      addCheck(plan, { label: "desktop TypeScript check", kind: "pnpm", args: ["--filter", "@xcoding/desktop", "check"] });
      addE2e(plan, cliStaticE2eTests, true);
      addReason(plan, "shared protocol source changed");
      continue;
    }

    if (filePath.startsWith("packages/client/src/")) {
      addCheck(plan, { label: "client TypeScript build", kind: "pnpm", args: ["--filter", "@xcoding/client", "build"] });
      addCheck(plan, { label: "CLI TypeScript check", kind: "pnpm", args: ["--filter", "@xcoding/cli", "check"] });
      addE2e(plan, cliStaticE2eTests, true);
      addReason(plan, "shared client source changed");
      continue;
    }

    const rustPackage = rustPackageFor(filePath);
    if (rustPackage) {
      addCheck(plan, { label: `${rustPackage} Rust tests`, kind: "cargo", args: ["test", "-p", rustPackage] });
      addE2e(plan, [], true);
      addReason(plan, `${rustPackage} source changed`);
      continue;
    }

    if (filePath.startsWith("apps/") || filePath.startsWith("packages/") || filePath.startsWith("crates/")) {
      markFull(plan, `${filePath} is outside a known incremental mapping`);
      continue;
    }

    if (filePath === "scripts/verify-affected.mjs") {
      addCheck(plan, { label: "affected-plan self-check", kind: "node", args: ["scripts/verify-affected.test.mjs"] });
      addReason(plan, "affected verification planner changed");
      continue;
    }

    if (filePath === "scripts/verify-affected.test.mjs") {
      addCheck(plan, { label: "affected-plan self-check", kind: "node", args: ["scripts/verify-affected.test.mjs"] });
      addReason(plan, "affected verification planner test changed");
      continue;
    }

    if (!filePath.startsWith("docs/") && filePath !== "README.md" && filePath !== "AGENTS.md" && filePath !== "CLAUDE.md") {
      markFull(plan, `${filePath} cannot be mapped safely`);
    }
  }

  if (plan.full) {
    plan.checks = [{ label: "full TypeScript check", kind: "pnpm-script", args: ["check"] }];
    plan.e2e = [];
    plan.e2eBuild = true;
  }
  plan.e2e = unique(plan.e2e).sort();
  return { files, ...plan };
}

function parseArguments(args) {
  const options = { build: true, dryRun: false, files: [], tests: [], base: "HEAD" };
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--dry-run") {
      options.dryRun = true;
    } else if (argument === "--no-build") {
      options.build = false;
    } else if (argument === "--file") {
      const filePath = args[index + 1];
      if (!filePath) throw new Error("--file requires a path");
      options.files.push(filePath);
      index += 1;
    } else if (argument === "--test") {
      const testName = args[index + 1];
      if (!testName) throw new Error("--test requires an e2e file name");
      options.tests.push(testName);
      index += 1;
    } else if (argument === "--base") {
      options.base = args[index + 1];
      if (!options.base) throw new Error("--base requires a git ref");
      index += 1;
    } else if (argument.startsWith("--")) {
      throw new Error(`unknown option: ${argument}`);
    } else {
      options.tests.push(argument);
    }
  }
  return options;
}

function captureProcess(command, args, options = {}) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, {
      cwd: repositoryRoot,
      env: process.env,
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
      ...options,
    });
    let stdout = "";
    let stderr = "";
    child.stdout?.setEncoding("utf8");
    child.stderr?.setEncoding("utf8");
    child.stdout?.on("data", (chunk) => { stdout += chunk; });
    child.stderr?.on("data", (chunk) => { stderr += chunk; });
    child.on("error", reject);
    child.on("close", (code) => resolvePromise({ code, stdout, stderr }));
  });
}

async function changedFiles(base) {
  const diff = await captureProcess("git", ["diff", "--name-only", "--diff-filter=ACMRD", base, "--"]);
  if (diff.code !== 0) throw new Error(diff.stderr.trim() || `git diff ${base} failed`);
  const untracked = await captureProcess("git", ["ls-files", "--others", "--exclude-standard"]);
  if (untracked.code !== 0) throw new Error(untracked.stderr.trim() || "git ls-files failed");
  return unique([...diff.stdout.split(/\r?\n/), ...untracked.stdout.split(/\r?\n/)]).filter(Boolean);
}

function commandLabel(command) {
  return `${command.kind} ${command.args.join(" ")}`;
}

async function runCommand(command) {
  const startedAt = performance.now();
  const cwd = command.cwd ? resolve(repositoryRoot, command.cwd) : repositoryRoot;
  let executable = command.kind === "node" ? process.execPath : command.kind === "cargo" ? "cargo" : "pnpm";
  let args = command.args;
  if (command.kind === "pnpm-script") {
    executable = "pnpm";
  }
  if (process.platform === "win32" && command.kind !== "node" && command.kind !== "cargo") {
    args = ["/d", "/s", "/c", "pnpm", ...args];
    executable = process.env.ComSpec || "cmd.exe";
  }
  console.log(`\n> ${command.label || commandLabel(command)}`);
  const result = await captureProcess(executable, args, { cwd, stdio: "inherit" });
  const seconds = ((performance.now() - startedAt) / 1000).toFixed(2);
  if (result.code !== 0) throw new Error(`${command.label || commandLabel(command)} failed with exit code ${result.code} after ${seconds}s`);
  console.log(`PASS ${command.label || commandLabel(command)} (${seconds}s)`);
}

function printPlan(plan, explicitTests = false) {
  console.log(`Changed files (${plan.files.length} relevant):`);
  for (const filePath of plan.files) console.log(`  ${filePath}`);
  if (plan.files.length === 0 && !explicitTests) {
    console.log("No relevant source or test changes detected; no verification command is needed.");
    return;
  }
  console.log(`\nPlan: ${plan.full ? "full fallback" : "affected scope"}`);
  for (const reason of plan.reasons) console.log(`  reason: ${reason}`);
  for (const check of plan.checks) console.log(`  check: ${check.label}`);
  if (plan.e2e.length > 0) console.log(`  e2e: ${plan.e2e.join(", ")} (build: ${plan.e2eBuild ? "yes" : "no"})`);
  else if (plan.e2eBuild) console.log("  e2e: all discovered tests (build: yes)");
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const files = options.files.length > 0 || options.tests.length > 0 ? options.files : await changedFiles(options.base);
  const plan = selectPlan(files);
  if (options.tests.length > 0) {
    plan.full = false;
    plan.checks = [];
    plan.e2e = options.tests;
    plan.e2eBuild = options.build;
    plan.reasons = ["explicit e2e test selection"];
  }
  printPlan(plan, options.tests.length > 0);
  if (options.dryRun || (plan.files.length === 0 && options.tests.length === 0)) return;

  for (const check of plan.checks) await runCommand(check);
  if (plan.e2e.length > 0 || plan.e2eBuild) {
    const args = ["tests/e2e/run.mjs"];
    if (!plan.e2eBuild) args.push("--no-build");
    args.push(...plan.e2e);
    await runCommand({ kind: "node", args, label: `e2e (${plan.e2e.length || "all"} tests)` });
  }
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
