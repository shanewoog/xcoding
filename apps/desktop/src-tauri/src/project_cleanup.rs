use std::fs;
use std::path::{Component, Path, PathBuf};

use xcoding_protocol::{
    CleanupProjectResult, ProjectCleanupItem, ProjectDir, ProjectDiskUsage, ProjectDiskUsageReport,
};

use crate::projects;

const CLEANABLE_DIR_NAMES: &[&str] = &[
    "node_modules",
    "target",
    ".next",
    ".nuxt",
    ".vite",
    ".turbo",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
];
const PROJECT_LOG_RELATIVE: &str = ".xcoding/logs";

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .trim_start_matches("//./")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn is_within_root(root: &Path, candidate: &Path) -> bool {
    let root = path_key(root);
    let candidate = path_key(candidate);
    candidate == root || candidate.starts_with(&(root + "/"))
}

fn relative_forward_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn is_cleanable_relative(relative: &str) -> bool {
    let normalized = relative.replace('\\', "/").to_ascii_lowercase();
    if normalized == PROJECT_LOG_RELATIVE {
        return true;
    }
    if normalized.starts_with(".xcoding/") || normalized.starts_with(".git/") {
        return false;
    }
    normalized
        .rsplit('/')
        .next()
        .is_some_and(|name| CLEANABLE_DIR_NAMES.contains(&name))
}

fn is_cleanable_path(project_root: &Path, path: &Path) -> bool {
    relative_forward_path(project_root, path)
        .as_deref()
        .is_some_and(is_cleanable_relative)
}

fn measure_directory(root: &Path) -> (u64, u64) {
    let mut total_bytes = 0_u64;
    let mut skipped_entries = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                skipped_entries = skipped_entries.saturating_add(1);
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    skipped_entries = skipped_entries.saturating_add(1);
                    continue;
                }
            };
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => {
                    skipped_entries = skipped_entries.saturating_add(1);
                    continue;
                }
            };
            if is_link_or_reparse(&metadata) {
                skipped_entries = skipped_entries.saturating_add(1);
                continue;
            }
            if metadata.is_file() {
                total_bytes = total_bytes.saturating_add(metadata.len());
            } else if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    (total_bytes, skipped_entries)
}

fn scan_project(project: &ProjectDir) -> ProjectDiskUsage {
    let project_root = PathBuf::from(&project.path);
    let root_metadata = fs::symlink_metadata(&project_root);
    if root_metadata
        .as_ref()
        .map(|metadata| is_link_or_reparse(metadata) || !metadata.is_dir())
        .unwrap_or(true)
    {
        return ProjectDiskUsage {
            project: project.clone(),
            total_bytes: 0,
            cleanable_bytes: 0,
            skipped_entries: 1,
            cleanable_items: Vec::new(),
        };
    }

    let mut total_bytes = 0_u64;
    let mut cleanable_bytes = 0_u64;
    let mut skipped_entries = 0_u64;
    let mut cleanable_items = Vec::new();
    let mut pending = vec![project_root.clone()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                skipped_entries = skipped_entries.saturating_add(1);
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    skipped_entries = skipped_entries.saturating_add(1);
                    continue;
                }
            };
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => {
                    skipped_entries = skipped_entries.saturating_add(1);
                    continue;
                }
            };
            if is_link_or_reparse(&metadata) {
                skipped_entries = skipped_entries.saturating_add(1);
                continue;
            }
            if metadata.is_file() {
                total_bytes = total_bytes.saturating_add(metadata.len());
                continue;
            }
            if !metadata.is_dir() {
                continue;
            }

            if is_cleanable_path(&project_root, &path) {
                let (bytes, skipped) = measure_directory(&path);
                total_bytes = total_bytes.saturating_add(bytes);
                cleanable_bytes = cleanable_bytes.saturating_add(bytes);
                skipped_entries = skipped_entries.saturating_add(skipped);
                if let Some(relative_path) = relative_forward_path(&project_root, &path) {
                    cleanable_items.push(ProjectCleanupItem {
                        relative_path,
                        bytes,
                    });
                }
            } else {
                pending.push(path);
            }
        }
    }

    cleanable_items.sort_by(|left, right| {
        right
            .bytes
            .cmp(&left.bytes)
            .then_with(|| left.relative_path.cmp(&right.relative_path))
    });
    ProjectDiskUsage {
        project: project.clone(),
        total_bytes,
        cleanable_bytes,
        skipped_entries,
        cleanable_items,
    }
}

/// Calculate approximate on-disk usage from file lengths and identify only the
/// explicitly supported generated dependency/build/cache directories.
pub fn scan_projects(workspace_home: &str) -> Result<ProjectDiskUsageReport, String> {
    let projects = projects::list_projects(workspace_home)?;
    let mut reports = Vec::with_capacity(projects.len());
    let mut total_bytes = 0_u64;
    let mut cleanable_bytes = 0_u64;
    for project in projects {
        let report = scan_project(&project);
        total_bytes = total_bytes.saturating_add(report.total_bytes);
        cleanable_bytes = cleanable_bytes.saturating_add(report.cleanable_bytes);
        reports.push(report);
    }
    Ok(ProjectDiskUsageReport {
        projects: reports,
        total_bytes,
        cleanable_bytes,
    })
}

fn same_project_path(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => path_key(&left) == path_key(&right),
        _ => path_key(left) == path_key(right),
    }
}

fn safe_cleanup_target(project_root: &Path, relative_path: &str) -> Option<PathBuf> {
    let relative = Path::new(relative_path);
    if relative_path.trim().is_empty() || relative.is_absolute() {
        return None;
    }
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    if !is_cleanable_relative(relative_path) {
        return None;
    }

    let target = project_root.join(relative);
    let metadata = fs::symlink_metadata(&target).ok()?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return None;
    }
    let canonical_root = fs::canonicalize(project_root).ok()?;
    let canonical_target = fs::canonicalize(&target).ok()?;
    is_within_root(&canonical_root, &canonical_target).then_some(target)
}

/// Remove every supported cleanable directory previously discovered inside one
/// workspace project. Unknown paths, symlinks and reparse points are rejected.
pub fn cleanup_project(
    workspace_home: &str,
    project_path: &str,
) -> Result<CleanupProjectResult, String> {
    let projects = projects::list_projects(workspace_home)?;
    let requested = Path::new(project_path.trim());
    let project = projects
        .into_iter()
        .find(|project| same_project_path(Path::new(&project.path), requested))
        .ok_or_else(|| {
            format!(
                "project is not registered under workspace home: {}",
                project_path.trim()
            )
        })?;
    let project_root = fs::canonicalize(&project.path).map_err(|error| error.to_string())?;
    let report = scan_project(&project);
    let mut removed_bytes = 0_u64;
    let mut removed_items = Vec::new();
    let mut failed_items = Vec::new();

    for item in report.cleanable_items {
        let Some(target) = safe_cleanup_target(&project_root, &item.relative_path) else {
            failed_items.push(item.relative_path);
            continue;
        };
        match fs::remove_dir_all(&target) {
            Ok(()) => {
                removed_bytes = removed_bytes.saturating_add(item.bytes);
                removed_items.push(item.relative_path);
            }
            Err(_) => failed_items.push(item.relative_path),
        }
    }

    Ok(CleanupProjectResult {
        removed_bytes,
        removed_items,
        failed_items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_home(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let home = std::env::temp_dir().join(format!("xcoding-project-cleanup-{label}-{nanos}"));
        fs::create_dir_all(&home).expect("temp home");
        home
    }

    fn write_file(path: &Path, bytes: usize) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent directory");
        }
        fs::write(path, vec![0_u8; bytes]).expect("fixture file");
    }

    #[test]
    fn scans_and_cleans_only_supported_generated_directories() {
        let home = temp_home("scan-clean");
        let project = home.join("demo");
        write_file(&project.join("src/main.rs"), 11);
        write_file(&project.join("dist/release.bin"), 13);
        write_file(&project.join(".git/objects/abc"), 17);
        write_file(&project.join(".xcoding/project-meta.json"), 31);
        write_file(&project.join("node_modules/pkg/index.js"), 19);
        write_file(&project.join("crates/app/target/out.bin"), 23);
        write_file(&project.join(".xcoding/logs/run.log"), 29);

        let report = scan_projects(home.to_str().expect("home path")).expect("scan projects");
        assert_eq!(report.projects.len(), 1);
        let usage = &report.projects[0];
        assert_eq!(usage.total_bytes, 143);
        assert_eq!(usage.cleanable_bytes, 71);
        assert_eq!(usage.cleanable_items.len(), 3);
        assert_eq!(report.total_bytes, 143);
        assert_eq!(report.cleanable_bytes, 71);

        let cleaned = cleanup_project(
            home.to_str().expect("home path"),
            project.to_str().expect("project path"),
        )
        .expect("cleanup project");
        assert_eq!(cleaned.removed_bytes, 71);
        assert!(cleaned.failed_items.is_empty());
        assert_eq!(cleaned.removed_items.len(), 3);
        assert!(!project.join("node_modules").exists());
        assert!(!project.join("crates/app/target").exists());
        assert!(!project.join(".xcoding/logs").exists());
        assert!(project.join("src/main.rs").exists());
        assert!(project.join("dist/release.bin").exists());
        assert!(project.join(".git/objects/abc").exists());
        assert!(project.join(".xcoding/project-meta.json").exists());

        let rescanned = scan_projects(home.to_str().expect("home path")).expect("rescan projects");
        assert_eq!(rescanned.projects[0].total_bytes, 72);
        assert_eq!(rescanned.projects[0].cleanable_bytes, 0);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn rejects_projects_outside_the_workspace_home() {
        let home = temp_home("home");
        let project = home.join("demo");
        fs::create_dir_all(&project).expect("project");
        let outside = temp_home("outside");
        let error = cleanup_project(
            home.to_str().expect("home path"),
            outside.to_str().expect("outside path"),
        )
        .expect_err("outside project must be rejected");
        assert!(error.contains("not registered"));
        let _ = fs::remove_dir_all(home);
        let _ = fs::remove_dir_all(outside);
    }
}
