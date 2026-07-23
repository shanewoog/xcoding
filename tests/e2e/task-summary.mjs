import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

function read(rel) {
  return readFileSync(resolve(root, rel), "utf8");
}

// Protocol surface
const protoTs = read("packages/protocol/src/index.ts");
assert.match(protoTs, /export type FileChangeKind/);
assert.match(protoTs, /file_changes\?: FileChangeSummary\[\]/);
assert.match(protoTs, /lines_added\?: number/);

const protoRs = read("crates/xcoding-protocol/src/lib.rs");
assert.match(protoRs, /enum FileChangeKind/);
assert.match(protoRs, /struct FileChangeSummary/);
assert.match(protoRs, /pub file_changes: Vec<FileChangeSummary>/);

// Core summary logic
const core = read("crates/xcoding-core/src/lib.rs");
assert.match(core, /fn classify_file_change/);
assert.match(core, /fn line_change_counts/);
assert.match(core, /latest_by_path/);

// CLI presentation + command
const cli = read("apps/cli/src/index.ts");
assert.match(cli, /function formatTaskSummary/);
assert.match(cli, /case "summary"/);
assert.match(cli, /session summary <session-id>/);
assert.match(cli, /\+\$\{added\}\/-\$\{removed\}/);

// Desktop presentation
const app = read("apps/desktop/src/App.tsx");
assert.match(app, /function formatTaskSummaryText/);
assert.match(app, /Copy summary/);
assert.match(app, /file-change-list/);
assert.match(app, /change-kind/);

const css = read("apps/desktop/src/styles.css");
assert.match(css, /\.file-change-list/);
assert.match(css, /\.change-kind\.created/);

// Pure format logic mirror (CLI shape)
function formatTaskSummary(summary) {
  const added = summary.lines_added ?? 0;
  const removed = summary.lines_removed ?? 0;
  const lines = [
    `Task complete: ${summary.changed_files.length} changed file(s), +${added}/-${removed} line(s); ` +
      `${summary.commands_succeeded}/${summary.commands_run} command(s) succeeded.`,
  ];
  for (const change of summary.file_changes ?? []) {
    lines.push(`  [${change.kind}] ${change.path} (+${change.lines_added}/-${change.lines_removed})`);
  }
  return lines.join("\n");
}

const rendered = formatTaskSummary({
  changed_files: ["src/a.rs", "src/b.rs"],
  file_changes: [
    { path: "src/a.rs", kind: "modified", lines_added: 1, lines_removed: 1 },
    { path: "src/b.rs", kind: "created", lines_added: 1, lines_removed: 0 },
  ],
  commands_run: 2,
  commands_succeeded: 1,
  commands_failed: 1,
  lines_added: 2,
  lines_removed: 1,
});
assert.match(rendered, /\+2\/-1 line/);
assert.match(rendered, /\[modified\] src\/a\.rs \(\+1\/-1\)/);
assert.match(rendered, /\[created\] src\/b\.rs \(\+1\/-0\)/);

console.log("Task summary checks passed.");
