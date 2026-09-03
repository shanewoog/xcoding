//! Project-rule loading and prompt context for the coding-agent loop.

use std::{collections::VecDeque, fs, path::Path};

use xcoding_mcp::{PluginConfig, load_plugin_config, user_skill_root};

/// Workspace-root rule files, in load order.
const RULE_FILES: [&str; 3] = ["AGENTS.md", "XCoding.md", ".xcoding/rules.md"];
const MAX_RULE_CHARS: usize = 20_000;
const MAX_RELEVANT_PATHS: usize = 40;
const SKETCH_MAX_DEPTH: usize = 2;
const MAX_SKILLS: usize = 32;
const MAX_SKILL_DESCRIPTION_CHARS: usize = 240;
const SKILLS_DIR: &str = ".xcoding/skills";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRule {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub path: String,
    pub source: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ContextSnapshot {
    pub project_rules: Vec<ProjectRule>,
    pub relevant_paths: Vec<String>,
    pub skills: Vec<SkillSummary>,
}

impl ContextSnapshot {
    pub fn load(workspace_root: &Path) -> Self {
        Self::load_with_plugin_config(workspace_root, &load_plugin_config())
    }

    pub fn load_with_plugin_config(workspace_root: &Path, plugin_config: &PluginConfig) -> Self {
        let project_rules = RULE_FILES
            .into_iter()
            .filter_map(|name| {
                let path = workspace_root.join(name);
                let content = fs::read_to_string(path).ok()?;
                let content = truncate_chars(content.trim(), MAX_RULE_CHARS, "project rule");
                if content.is_empty() {
                    return None;
                }
                Some(ProjectRule {
                    path: name.to_owned(),
                    content,
                })
            })
            .collect();

        Self {
            project_rules,
            relevant_paths: workspace_path_sketch(workspace_root),
            skills: load_skill_catalog(workspace_root, plugin_config),
        }
    }

    /// Build the system prompt for the active mode (`ask`, `auto-edit`, or `full-auto`).
    pub fn system_prompt(&self, mode: &str) -> String {
        let mut prompt = format!(
            "You are XCoding, a local coding agent for a software workspace. \
When repository facts are needed, use tools before answering. Never claim a file was inspected unless a tool result contains it. \
Available tools: list_dir, read_file, search_code, load_skill, apply_patch, run_command, git_status, git_diff, git_log, git_show, git_add, git_commit, git_push, git_fetch, git_pull, browser_state, update_plan. \
Current mode: {mode}. \
In ask mode, propose writes and wait for required approval. In auto-edit mode, ordinary file patches and allowlisted safe commands may apply without approval; high-risk writes and non-allowlisted commands still require user approval. \
In full-auto mode, every permitted write and command runs without approval; hard-denied destructive commands are still blocked, so act with extra care. \
Prefer minimal, scoped changes. Do not invent secrets or commit credentials. If apply_patch fails with a patch conflict, re-read the file and retry with updated old_text; do not force-write without matching the current contents. \
When several independent read-only lookups are needed, request them as parallel tool calls in one turn instead of one call per turn. \
For any request that needs more than one action, call update_plan first with your own concrete steps for this task, and choose the number of steps the task actually needs instead of a fixed count. Name real work such as the files, symbols, or commands involved, not generic phases. Call update_plan again as you go to mark the finished step done and the next one in_progress, keeping at most one step in_progress. Skip update_plan for a trivial single-action request. \
When starting a local service, launch it with run_command background=true plus ready_port and, when it needs its own directory, cwd. A ready=true result already proves the service is up, so do not add health-check commands after it. \
Once the user's stated goal is met, answer and stop. Do not extend a finished request into further exploration such as unrelated credentials, auth flows, or code reading; if follow-up work looks useful, name it in the answer and let the user decide. \
When giving the user a URL to open, write it as bare text. Do not wrap it in backticks or a fenced code block, because only unwrapped URLs render as clickable links in the desktop UI. \
When a workspace skill matches the task, call load_skill with its name before following its instructions."
        );

        #[cfg(target_os = "windows")]
        prompt.push_str(
            "\n\nRuntime environment: Windows. run_command executes one program with a separate argument vector; do not invoke cmd, PowerShell, bash, or another shell wrapper. \
Use Windows-compatible direct commands: rg for text search, where.exe to locate an executable, tasklist for processes, and netstat -ano for ports. \
Do not use Unix-only diagnostics such as ps -ef, which, lsof, grep, or netstat -tln/-tlnp. \
For GitNexus, prefer built-in code-analysis tools. For a CLI fallback, invoke gitnexus directly with --repo when needed. Never run node .gitnexus/run.cjs unless that exact file was verified to exist; a missing runner is not a reason to retry it.",
        );

        if !self.project_rules.is_empty() {
            prompt.push_str("\n\nProject rules (follow these for this workspace):\n");
            for rule in &self.project_rules {
                prompt.push_str(&format!("\n--- {} ---\n{}\n", rule.path, rule.content));
            }
        }

        if !self.skills.is_empty() {
            prompt.push_str(
                "\n\nWorkspace skills (catalog only; call load_skill to load full instructions):\n",
            );
            for skill in &self.skills {
                prompt.push_str(&format!("- {}: {}\n", skill.name, skill.description));
            }
        }

        if !self.relevant_paths.is_empty() {
            prompt.push_str(
                "\n\nWorkspace sketch (shallow paths for orientation; still use tools before quoting file contents):\n",
            );
            for path in &self.relevant_paths {
                prompt.push_str("- ");
                prompt.push_str(path);
                prompt.push('\n');
            }
        }

        prompt
    }
}

fn load_skill_catalog(workspace_root: &Path, plugin_config: &PluginConfig) -> Vec<SkillSummary> {
    let mut roots = vec![
        (
            "workspace",
            workspace_root.join(SKILLS_DIR),
            ".xcoding/skills".to_owned(),
        ),
        ("user", user_skill_root(), "~/.xcoding/skills".to_owned()),
    ];
    let mut seen = std::collections::HashSet::new();
    let mut skills = Vec::new();

    for (source, skills_root, display_root) in roots.drain(..) {
        let Ok(entries) = fs::read_dir(&skills_root) else {
            continue;
        };

        let mut folders = entries
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_type()
                    .map(|file_type| file_type.is_dir())
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();
        folders.sort_by_key(|entry| entry.file_name());

        for entry in folders {
            if skills.len() >= MAX_SKILLS {
                break;
            }
            let folder_name = entry.file_name().to_string_lossy().into_owned();
            if !is_valid_skill_name(&folder_name) {
                continue;
            }
            if !plugin_config
                .skill_enabled
                .get(&format!("{source}:{folder_name}"))
                .copied()
                .unwrap_or(true)
            {
                continue;
            }
            if !seen.insert(folder_name.clone()) {
                continue;
            }
            let skill_path = entry.path().join("SKILL.md");
            let Ok(raw) = fs::read_to_string(&skill_path) else {
                continue;
            };
            let parsed = parse_skill_document(&folder_name, &raw);
            skills.push(SkillSummary {
                // Catalog key is always the folder name so load_skill arguments stay stable.
                name: folder_name.clone(),
                description: truncate_chars(
                    parsed.description.trim(),
                    MAX_SKILL_DESCRIPTION_CHARS,
                    "skill description",
                ),
                path: format!("{display_root}/{folder_name}/SKILL.md"),
                source: source.to_owned(),
            });
        }
    }
    skills
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedSkill {
    name: String,
    description: String,
    body: String,
}

fn parse_skill_document(folder_name: &str, raw: &str) -> ParsedSkill {
    let normalized = raw.replace("\r\n", "\n");
    let (name, description, body) = if let Some(rest) = normalized.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            let frontmatter = &rest[..end];
            let body = rest[end + "\n---\n".len()..].to_owned();
            let mut name = None;
            let mut description = None;
            for line in frontmatter.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim();
                    let value = value.trim().trim_matches('"').trim_matches('\'').to_owned();
                    match key {
                        "name" if !value.is_empty() => name = Some(value),
                        "description" if !value.is_empty() => description = Some(value),
                        _ => {}
                    }
                }
            }
            (
                name.unwrap_or_else(|| folder_name.to_owned()),
                description.unwrap_or_else(|| fallback_description(&body)),
                body,
            )
        } else {
            (
                folder_name.to_owned(),
                fallback_description(&normalized),
                normalized.clone(),
            )
        }
    } else {
        (
            folder_name.to_owned(),
            fallback_description(&normalized),
            normalized.clone(),
        )
    };

    ParsedSkill {
        name,
        description,
        body,
    }
}

fn fallback_description(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .or_else(|| {
            body.lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(|line| line.trim_start_matches('#').trim())
        })
        .unwrap_or("Workspace skill")
        .to_owned()
}

fn is_valid_skill_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    name.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
}

fn truncate_chars(content: &str, max_chars: usize, label: &str) -> String {
    if content.chars().count() <= max_chars {
        return content.to_owned();
    }
    let mut truncated = content.chars().take(max_chars).collect::<String>();
    truncated.push_str(&format!("\n...[truncated {label}]..."));
    truncated
}

fn workspace_path_sketch(workspace_root: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    let mut pending = VecDeque::from([(workspace_root.to_path_buf(), 0usize)]);

    while let Some((directory, depth)) = pending.pop_front() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        let mut children: Vec<_> = entries.filter_map(Result::ok).collect();
        children.sort_by_key(|entry| entry.file_name());

        for entry in children {
            if paths.len() >= MAX_RELEVANT_PATHS {
                return paths;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            if file_type.is_dir() && is_ignored_sketch_directory(&name) {
                continue;
            }

            let absolute = entry.path();
            let Some(relative) = relative_path_string(workspace_root, &absolute) else {
                continue;
            };
            if file_type.is_dir() {
                paths.push(format!("{relative}/"));
                if depth + 1 <= SKETCH_MAX_DEPTH {
                    pending.push_back((absolute, depth + 1));
                }
            } else if file_type.is_file() {
                paths.push(relative);
            }
        }
    }

    paths
}

fn relative_path_string(workspace_root: &Path, absolute: &Path) -> Option<String> {
    absolute
        .strip_prefix(workspace_root)
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
}

fn is_ignored_sketch_directory(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_string_lossy().as_ref(),
        ".git"
            | ".xcoding"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".next"
            | "coverage"
            | "__pycache__"
            | ".venv"
            | "venv"
            | ".cargo"
    )
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn temp_workspace(label: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock works")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("xcoding-context-{label}-{unique}"));
        fs::create_dir_all(&root).expect("workspace creates");
        root
    }

    #[test]
    fn loads_root_project_rules_into_the_system_prompt() {
        let root = temp_workspace("agents");
        fs::write(root.join("AGENTS.md"), "Run focused tests.").expect("rule writes");

        let context = ContextSnapshot::load(&root);
        assert_eq!(context.project_rules.len(), 1);
        let prompt = context.system_prompt("ask");
        assert!(prompt.contains("Run focused tests."));
        assert!(prompt.contains("apply_patch"));
        assert!(prompt.contains("load_skill"));
        assert!(prompt.contains("patch conflict"));
        assert!(prompt.contains("Current mode: ask"));
        assert!(prompt.contains("AGENTS.md"));
        assert!(prompt.contains("Workspace sketch"));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn system_prompt_asks_for_parallel_read_only_tool_calls() {
        let root = temp_workspace("parallel-tool-guidance");

        let prompt = ContextSnapshot::load(&root).system_prompt("full-auto");
        assert!(prompt.contains("independent read-only lookups"));
        assert!(prompt.contains("parallel tool calls in one turn"));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn system_prompt_asks_the_model_to_author_its_own_plan() {
        let root = temp_workspace("plan-guidance");

        let prompt = ContextSnapshot::load(&root).system_prompt("full-auto");
        assert!(prompt.contains("update_plan"));
        // The step count belongs to the model, not to a hardcoded scaffold.
        assert!(prompt.contains("the number of steps the task actually needs"));
        assert!(prompt.contains("at most one step in_progress"));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn system_prompt_bounds_service_startup_and_finished_work() {
        let root = temp_workspace("service-startup-guidance");

        let prompt = ContextSnapshot::load(&root).system_prompt("full-auto");
        // A started service must be proven by the launch itself, not by follow-up
        // health-check commands.
        assert!(prompt.contains("ready_port"));
        assert!(prompt.contains("do not add health-check commands after it"));
        // And a met goal has to end the turn instead of drifting into unrelated
        // exploration.
        assert!(prompt.contains("answer and stop"));
        // A URL the user is meant to open has to stay clickable in the desktop UI.
        assert!(prompt.contains("Do not wrap it in backticks or a fenced code block"));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn system_prompt_gives_windows_command_guidance() {
        let root = temp_workspace("windows-command-guidance");

        let prompt = ContextSnapshot::load(&root).system_prompt("ask");
        assert!(prompt.contains("Runtime environment: Windows"));
        assert!(prompt.contains("netstat -ano"));
        assert!(prompt.contains("ps -ef"));
        assert!(prompt.contains("netstat -tln/-tlnp"));
        assert!(prompt.contains("invoke gitnexus directly"));
        assert!(prompt.contains("Never run node .gitnexus/run.cjs unless"));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn loads_dot_xcoding_rules_file() {
        let root = temp_workspace("dot-rules");
        fs::create_dir_all(root.join(".xcoding")).expect("dir creates");
        fs::write(root.join(".xcoding/rules.md"), "Prefer ASCII comments.").expect("rule writes");

        let context = ContextSnapshot::load(&root);
        assert_eq!(context.project_rules.len(), 1);
        assert_eq!(context.project_rules[0].path, ".xcoding/rules.md");
        assert!(
            context
                .system_prompt("auto-edit")
                .contains("Prefer ASCII comments.")
        );

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn loads_all_supported_rule_files_in_order() {
        let root = temp_workspace("all-rules");
        fs::create_dir_all(root.join(".xcoding")).expect("dir creates");
        fs::write(root.join("AGENTS.md"), "agents").expect("write");
        fs::write(root.join("XCoding.md"), "xcoding").expect("write");
        fs::write(root.join(".xcoding/rules.md"), "rules").expect("write");

        let context = ContextSnapshot::load(&root);
        let paths: Vec<_> = context
            .project_rules
            .iter()
            .map(|rule| rule.path.as_str())
            .collect();
        assert_eq!(paths, vec!["AGENTS.md", "XCoding.md", ".xcoding/rules.md"]);

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn truncates_oversized_rule_content() {
        let root = temp_workspace("truncate");
        let oversized = "x".repeat(MAX_RULE_CHARS + 50);
        fs::write(root.join("AGENTS.md"), &oversized).expect("write");

        let context = ContextSnapshot::load(&root);
        assert_eq!(context.project_rules.len(), 1);
        assert!(
            context.project_rules[0]
                .content
                .contains("[truncated project rule]")
        );
        assert!(context.project_rules[0].content.chars().count() < oversized.chars().count());

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn sketches_shallow_workspace_paths_and_skips_ignored_dirs() {
        let root = temp_workspace("sketch");
        fs::create_dir_all(root.join("src/nested")).expect("src creates");
        fs::create_dir_all(root.join("node_modules/pkg")).expect("node_modules creates");
        fs::create_dir_all(root.join("target/debug")).expect("target creates");
        fs::write(root.join("package.json"), "{}\n").expect("package writes");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("main writes");
        fs::write(root.join("src/nested/mod.rs"), "// nested\n").expect("nested writes");
        fs::write(root.join("node_modules/pkg/index.js"), "export {}\n").expect("nm writes");

        let context = ContextSnapshot::load(&root);
        assert!(
            context
                .relevant_paths
                .iter()
                .any(|path| path == "package.json")
        );
        assert!(context.relevant_paths.iter().any(|path| path == "src/"));
        assert!(
            context
                .relevant_paths
                .iter()
                .any(|path| path == "src/main.rs")
        );
        assert!(
            context
                .relevant_paths
                .iter()
                .any(|path| path == "src/nested/")
        );
        assert!(
            context
                .relevant_paths
                .iter()
                .any(|path| path == "src/nested/mod.rs")
        );
        assert!(
            !context
                .relevant_paths
                .iter()
                .any(|path| path.contains("node_modules"))
        );
        assert!(
            !context
                .relevant_paths
                .iter()
                .any(|path| path.contains("target"))
        );

        let prompt = context.system_prompt("ask");
        assert!(prompt.contains("Workspace sketch"));
        assert!(prompt.contains("src/main.rs"));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn loads_workspace_skills_into_catalog_and_prompt() {
        let root = temp_workspace("skills");
        let skill_dir = root.join(".xcoding/skills/hello-style");
        fs::create_dir_all(&skill_dir).expect("skill dir");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: hello-style\ndescription: Prefer concise Chinese summaries\n---\n# Hello Style\nAlways end with DONE.\n",
        )
        .expect("skill writes");
        fs::create_dir_all(root.join(".xcoding/skills/no-md")).expect("empty skill");
        fs::create_dir_all(root.join(".xcoding/skills/../escape")).ok();

        let context = ContextSnapshot::load(&root);
        assert_eq!(context.skills.len(), 1);
        assert_eq!(context.skills[0].name, "hello-style");
        assert_eq!(
            context.skills[0].description,
            "Prefer concise Chinese summaries"
        );
        assert_eq!(
            context.skills[0].path,
            ".xcoding/skills/hello-style/SKILL.md"
        );

        let prompt = context.system_prompt("ask");
        assert!(prompt.contains("Workspace skills"));
        assert!(prompt.contains("hello-style"));
        assert!(prompt.contains("Prefer concise Chinese summaries"));
        assert!(prompt.contains("load_skill"));

        fs::remove_dir_all(root).expect("workspace removes");
    }

    #[test]
    fn skill_without_frontmatter_uses_folder_and_body() {
        let root = temp_workspace("skills-fallback");
        let skill_dir = root.join(".xcoding/skills/plain-skill");
        fs::create_dir_all(&skill_dir).expect("skill dir");
        fs::write(
            skill_dir.join("SKILL.md"),
            "# Title\nUse snake_case for helpers.\n",
        )
        .expect("skill writes");

        let context = ContextSnapshot::load(&root);
        assert_eq!(context.skills.len(), 1);
        assert_eq!(context.skills[0].name, "plain-skill");
        assert_eq!(context.skills[0].description, "Use snake_case for helpers.");

        fs::remove_dir_all(root).expect("workspace removes");
    }
}
