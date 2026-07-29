use std::fs;
use std::path::{Path, PathBuf};

use pinyin::ToPinyin;
use serde_json::json;
use xcoding_protocol::{
    CreateProjectParams, CreateProjectResult, ImportProjectParams, ImportProjectResult, ProjectDir,
};

fn is_cjk(ch: char) -> bool {
    matches!(
        ch,
        '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
    )
}

/// Turn a display project name into a lowercase English-ish directory slug.
/// CJK characters become pinyin tokens; ASCII words stay lowercase.
pub fn project_dir_slug(display_name: &str) -> Result<String, String> {
    let trimmed = display_name.trim();
    if trimmed.is_empty() {
        return Err("project name is required".to_owned());
    }

    let mut tokens: Vec<String> = Vec::new();
    let mut buf = String::new();

    let flush = |buf: &mut String, tokens: &mut Vec<String>| {
        if !buf.is_empty() {
            tokens.push(std::mem::take(buf));
        }
    };

    for ch in trimmed.chars() {
        if is_cjk(ch) {
            flush(&mut buf, &mut tokens);
            if let Some(py) = ch.to_pinyin() {
                tokens.push(py.plain().to_string());
            }
            continue;
        }
        if ch.is_ascii_alphanumeric() {
            buf.push(ch.to_ascii_lowercase());
            continue;
        }
        if matches!(ch, ' ' | '\t' | '-' | '_' | '.' | '/' | '\\') {
            flush(&mut buf, &mut tokens);
            continue;
        }
        flush(&mut buf, &mut tokens);
    }
    flush(&mut buf, &mut tokens);

    let slug = tokens
        .into_iter()
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        return Err("could not derive a directory name from the project name".to_owned());
    }
    if slug == "." || slug == ".." {
        return Err("invalid project directory name".to_owned());
    }
    if slug
        .chars()
        .any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        return Err("invalid project directory name".to_owned());
    }
    Ok(slug)
}

fn meta_path(project_path: &Path) -> PathBuf {
    project_path.join(".xcoding").join("project-meta.json")
}

fn read_title(project_path: &Path, fallback: &str) -> String {
    let path = meta_path(project_path);
    let Ok(raw) = fs::read_to_string(path) else {
        return fallback.to_owned();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return fallback.to_owned();
    };
    value
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

fn write_title(project_path: &Path, title: &str) -> Result<(), String> {
    let meta_dir = project_path.join(".xcoding");
    fs::create_dir_all(&meta_dir).map_err(|error| error.to_string())?;
    let meta = json!({ "title": title });
    fs::write(
        meta_path(project_path),
        serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn project_from_path(path: PathBuf) -> Option<ProjectDir> {
    if !path.is_dir() {
        return None;
    }
    let dir_name = path.file_name()?.to_string_lossy().to_string();
    if dir_name.starts_with('.') {
        return None;
    }
    let title = read_title(&path, &dir_name);
    Some(ProjectDir {
        path: path.to_string_lossy().to_string(),
        dir_name,
        title,
    })
}

fn normalize_path_key(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let trimmed = raw
        .trim_start_matches("//?/")
        .trim_start_matches("//./")
        .trim_end_matches('/');
    trimmed.to_ascii_lowercase()
}

fn resolve_existing_dir(path: &Path) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err(format!("path does not exist: {}", path.display()));
    }
    if !path.is_dir() {
        return Err(format!("path is not a directory: {}", path.display()));
    }
    fs::canonicalize(path).map_err(|error| error.to_string())
}

fn is_within_home(home: &Path, candidate: &Path) -> bool {
    let home_key = normalize_path_key(home);
    let candidate_key = normalize_path_key(candidate);
    candidate_key == home_key || candidate_key.starts_with(&(home_key.clone() + "/"))
}

fn direct_child_project(home: &Path, source: &Path) -> Result<PathBuf, String> {
    let home_key = normalize_path_key(home);
    let source_key = normalize_path_key(source);
    if source_key == home_key {
        return Err("select a project folder inside the workspace, not the workspace root".to_owned());
    }
    if !source_key.starts_with(&(home_key.clone() + "/")) {
        return Err(format!(
            "selected folder is not inside workspace home: {}",
            source.display()
        ));
    }
    let rel = &source_key[home_key.len() + 1..];
    let mut parts = rel.split('/').filter(|part| !part.is_empty());
    let Some(first) = parts.next() else {
        return Err("select a project folder inside the workspace".to_owned());
    };
    if first.starts_with('.') {
        return Err("hidden folders cannot be added as projects".to_owned());
    }
    // Only top-level workspace folders are project roots.
    if parts.next().is_some() {
        return Err(
            "select a top-level project folder under the workspace home (not a nested path)"
                .to_owned(),
        );
    }
    // Preserve the real directory name from the source path (not the lowercased key).
    let dir_name = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "invalid project folder name".to_owned())?;
    Ok(home.join(dir_name))
}

fn unique_destination(home: &Path, dir_name: &str) -> PathBuf {
    let base = home.join(dir_name);
    if !base.exists() {
        return base;
    }
    for index in 2..10_000 {
        let candidate = home.join(format!("{dir_name}-{index}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    home.join(format!(
        "{dir_name}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ))
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|error| error.to_string())?;
    let entries = fs::read_dir(src).map_err(|error| error.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target).map_err(|error| error.to_string())?;
        }
        // Skip symlinks and other special files for safety.
    }
    Ok(())
}

/// Dedicated workspace for unbound chat sessions (not a project).
/// Prefer `{workspace_home}/.xcoding-chat`; fall back to the app config dir.
pub fn ensure_chat_workspace(workspace_home: Option<&str>) -> Result<String, String> {
    let root = match workspace_home.map(str::trim).filter(|s| !s.is_empty()) {
        Some(home) => PathBuf::from(home).join(".xcoding-chat"),
        None => xcoding_providers::user_config_dir().join(".xcoding-chat"),
    };
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    Ok(root.to_string_lossy().to_string())
}

pub fn list_projects(workspace_home: &str) -> Result<Vec<ProjectDir>, String> {
    let home = PathBuf::from(workspace_home.trim());
    if workspace_home.trim().is_empty() {
        return Ok(Vec::new());
    }
    if !home.is_dir() {
        return Err(format!(
            "workspace home is not a directory: {}",
            home.display()
        ));
    }
    let mut projects = Vec::new();
    let entries = fs::read_dir(&home).map_err(|error| error.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        if let Some(project) = project_from_path(entry.path()) {
            projects.push(project);
        }
    }
    projects.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    Ok(projects)
}

pub fn create_project(params: CreateProjectParams) -> Result<CreateProjectResult, String> {
    let home_raw = params.workspace_home.trim();
    let name = params.name.trim();
    if home_raw.is_empty() {
        return Err("workspace home is required".to_owned());
    }
    if name.is_empty() {
        return Err("project name is required".to_owned());
    }
    let home = PathBuf::from(home_raw);
    if !home.is_dir() {
        return Err(format!(
            "workspace home is not a directory: {}",
            home.display()
        ));
    }
    let dir_name = project_dir_slug(name)?;
    let path = home.join(&dir_name);
    if path.exists() {
        return Err(format!(
            "project directory already exists: {}",
            path.display()
        ));
    }
    fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    write_title(&path, name)?;

    Ok(CreateProjectResult {
        project: ProjectDir {
            path: path.to_string_lossy().to_string(),
            dir_name,
            title: name.to_owned(),
        },
    })
}

/// Add an existing folder as a project under the workspace home.
/// - Inside workspace (top-level): reuse it; caller should ignore if already listed.
/// - Outside workspace: copy the folder into the workspace home.
pub fn import_project(params: ImportProjectParams) -> Result<ImportProjectResult, String> {
    let home_raw = params.workspace_home.trim();
    let source_raw = params.source_path.trim();
    if home_raw.is_empty() {
        return Err("workspace home is required".to_owned());
    }
    if source_raw.is_empty() {
        return Err("source path is required".to_owned());
    }
    let home = resolve_existing_dir(Path::new(home_raw))?;
    let source = resolve_existing_dir(Path::new(source_raw))?;

    if is_within_home(&home, &source) {
        let project_path = direct_child_project(&home, &source)?;
        let Some(project) = project_from_path(project_path) else {
            return Err("selected folder cannot be used as a project".to_owned());
        };
        return Ok(ImportProjectResult {
            project,
            already_existed: true,
            copied: false,
        });
    }

    let original_name = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "invalid source folder name".to_owned())?;
    if original_name.starts_with('.') {
        return Err("hidden folders cannot be imported as projects".to_owned());
    }
    let dest = unique_destination(&home, &original_name);
    copy_dir_recursive(&source, &dest)?;
    if !meta_path(&dest).exists() {
        let _ = write_title(&dest, &original_name);
    }
    let Some(project) = project_from_path(dest) else {
        return Err("failed to register copied project".to_owned());
    };
    Ok(ImportProjectResult {
        project,
        already_existed: false,
        copied: true,
    })
}

pub fn pick_directory(title: Option<String>) -> Result<Option<String>, String> {
    let mut dialog = rfd::FileDialog::new();
    if let Some(title) = title.map(|value| value.trim().to_owned()).filter(|value| !value.is_empty())
    {
        dialog = dialog.set_title(title);
    }
    Ok(dialog
        .pick_folder()
        .map(|path| path.to_string_lossy().to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        copy_dir_recursive, direct_child_project, is_within_home, project_dir_slug,
        unique_destination,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("xcoding-projects-{label}-{nanos}"));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn slugs_ascii_names() {
        assert_eq!(project_dir_slug("My App").unwrap(), "my-app");
        assert_eq!(project_dir_slug("Hello_World").unwrap(), "hello-world");
    }

    #[test]
    fn slugs_chinese_to_pinyin_phrase() {
        let slug = project_dir_slug("嘟嘟桌面").unwrap();
        assert_eq!(slug, "du-du-zhuo-mian");
        assert!(slug.chars().all(|ch| ch.is_ascii_lowercase() || ch == '-'));
    }

    #[test]
    fn slugs_mixed_names() {
        assert_eq!(project_dir_slug("智能 ETF").unwrap(), "zhi-neng-etf");
    }

    #[test]
    fn detects_paths_inside_workspace_home() {
        let home = temp_dir("home");
        let child = home.join("demo");
        fs::create_dir_all(&child).unwrap();
        assert!(is_within_home(&home, &child));
        assert!(!is_within_home(&home, &temp_dir("outside")));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn only_allows_top_level_workspace_children() {
        let home = temp_dir("top");
        let child = home.join("app");
        let nested = child.join("src");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(direct_child_project(&home, &child).unwrap(), child);
        assert!(direct_child_project(&home, &nested).is_err());
        assert!(direct_child_project(&home, &home).is_err());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn unique_destination_avoids_existing_names() {
        let home = temp_dir("unique");
        fs::create_dir_all(home.join("app")).unwrap();
        let next = unique_destination(&home, "app");
        assert_eq!(next.file_name().unwrap().to_string_lossy(), "app-2");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn copy_dir_recursive_copies_files() {
        let src = temp_dir("src");
        let dst_parent = temp_dir("dst-parent");
        let dst = dst_parent.join("copy");
        fs::write(src.join("a.txt"), b"hello").unwrap();
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub").join("b.txt"), b"world").unwrap();
        copy_dir_recursive(&src, &dst).unwrap();
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "hello");
        assert_eq!(
            fs::read_to_string(dst.join("sub").join("b.txt")).unwrap(),
            "world"
        );
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst_parent);
    }
}
