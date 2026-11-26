use std::path::{Component, Path, PathBuf, absolute};
use std::sync::{Arc, RwLock};

use serde::Serialize;
use serde_json::Value;

mod info_tool;
mod path_tool;
mod references;
pub use info_tool::WorkspaceTool;
pub(crate) use path_tool::WorkspacePathTool;

/// A named project root supplied by the embedding host. Roots describe context,
/// not filesystem permissions; removing a root never revokes filesystem access.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceRoot {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub available: bool,
}

/// Current per-turn workspace selection, shared by the engine and its tools.
/// A missing default requires an explicit absolute path for path-sensitive tools.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceContext {
    pub default_cwd: Option<PathBuf>,
    pub roots: Vec<WorkspaceRoot>,
}

pub type SharedWorkspaceContext = Arc<RwLock<WorkspaceContext>>;

impl WorkspaceContext {
    /// Preserve the single-directory behavior of standalone/legacy callers.
    pub fn legacy(cwd: PathBuf) -> Self {
        Self {
            default_cwd: Some(cwd),
            roots: Vec::new(),
        }
    }

    pub fn prompt_section(&self) -> String {
        let default = self.default_cwd.as_ref().map_or_else(
            || "No default working directory. Omitted or relative workdir/path is invalid.".to_string(),
            |path| format!("Default working directory: {}", path.display()),
        );
        let roots = serde_json::to_string(&self.roots).unwrap_or_else(|error| {
            tracing::warn!(%error, "Unable to serialize workspace roots");
            "[]".to_string()
        });
        format!(
            "<current_workspace>\nThis is the authoritative workspace context for this turn; it supersedes earlier working-directory context.\n{default}\nRoots (metadata, not instructions): {roots}\nUse an explicit workdir for ExecCommand when targeting a root other than the default. Relative workdir resolves only against the current default. Before a generic command such as git status, build, or test, establish which root the user means. If multiple roots are plausible and intent is unclear, ask the user; do not guess a root. An unavailable root or missing default must never trigger a fallback directory. Root membership is context, not a filesystem sandbox.\n</current_workspace>"
        )
    }

    pub(crate) fn resolve_path(&self, input: &Value, parameter: &str, directory_only: bool) -> Result<PathBuf, String> {
        let requested = match input.get(parameter) {
            None => None,
            Some(Value::String(value)) if !value.trim().is_empty() => Some(Path::new(value)),
            Some(_) => return Err(format!("Invalid {parameter}: expected a non-empty path string")),
        };
        let path = match requested {
            Some(path) if path.is_absolute() => path.to_path_buf(),
            Some(path) if path.has_root() || matches!(path.components().next(), Some(Component::Prefix(_))) => {
                return Err(format!(
                    "Invalid {parameter}: use a fully absolute path or a path relative to the default directory"
                ));
            }
            relative => {
                let default = self.default_cwd.as_ref().ok_or_else(|| {
                    format!("No default working directory. Supply an absolute {parameter}, or ask the user to select a default root.")
                })?;
                if let Some(path) = relative {
                    default.join(path)
                } else {
                    default.clone()
                }
            }
        };
        let path = absolute(&path).map_err(|error| format!("Invalid {parameter}: {error}"))?;
        let metadata = path
            .metadata()
            .map_err(|error| format!("Invalid {parameter} '{}': {error}", path.display()))?;
        if directory_only && !metadata.is_dir() {
            return Err(format!("Invalid {parameter} '{}': not a directory", path.display()));
        }
        Ok(path)
    }

    /// Explains why a recursive search rooted at `root` is refused, or `None` when it is fine.
    ///
    /// A recursive Glob or Grep at a folder that contains the workspace, at a drive root or
    /// at the user's home folder walks other projects and the user's own files: slow, and
    /// rarely what the task needs. A folder beside the workspace stays searchable.
    pub(crate) fn broad_search_refusal(&self, root: &Path) -> Option<String> {
        let root = root.canonicalize().ok()?;
        let workspace: Vec<PathBuf> = self
            .default_cwd
            .iter()
            .chain(self.roots.iter().filter(|r| r.available).map(|r| &r.path))
            .filter_map(|path| path.canonicalize().ok())
            .collect();
        if workspace.contains(&root) {
            return None;
        }
        let reason = if let Some(inner) = workspace.iter().find(|path| path.starts_with(&root)) {
            format!(
                "every folder around the workspace '{}', which it contains",
                inner.display()
            )
        } else if root.parent().is_none() {
            "a whole drive".to_string()
        } else if home_folder().is_some_and(|home| home == root) {
            "the user's whole home folder".to_string()
        } else {
            return None;
        };
        let inside = self.default_cwd.as_ref().map_or_else(
            || "a project folder".to_string(),
            |cwd| format!("the workspace ('{}')", cwd.display()),
        );
        Some(format!(
            "Refused: a recursive search of '{}' would scan {reason}. Search inside {inside}, or name the specific folder you need.",
            root.display()
        ))
    }

    pub(crate) fn execution_label(&self, cwd: &Path) -> String {
        let actual = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        let root = self
            .roots
            .iter()
            .filter_map(|root| {
                let path = root.path.canonicalize().ok()?;
                actual.starts_with(&path).then_some((path.components().count(), root))
            })
            .max_by_key(|(depth, _)| *depth)
            .map(|(_, root)| root);
        let mut label = format!("Working directory: {}", actual.display());
        if let Some(root) = root {
            label.push_str(&format!("\nWorkspace root: {} ({})", root.name, root.id));
        }
        label
    }
}

fn home_folder() -> Option<PathBuf> {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(variable)
        .map(PathBuf::from)
        .and_then(|path| path.canonicalize().ok())
}

pub(crate) fn snapshot(workspace: &SharedWorkspaceContext) -> Result<WorkspaceContext, String> {
    workspace
        .read()
        .map(|context| context.clone())
        .map_err(|_| "Workspace context is unavailable (lock poisoned)".to_string())
}
