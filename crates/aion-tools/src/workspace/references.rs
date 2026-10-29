use std::path::{Component, Path, PathBuf};

use super::WorkspaceContext;

impl WorkspaceContext {
    /// Resolve only explicit workspace references; ordinary path/prose semantics are unchanged.
    pub(super) fn resolve_reference(&self, value: &str) -> Result<Option<PathBuf>, String> {
        let Some(reference) = value.strip_prefix('@') else {
            return Ok(None);
        };
        let (selector, suffix) = if let Some(braced) = reference.strip_prefix('{') {
            let end = braced
                .find('}')
                .ok_or("Invalid workspace reference: missing closing brace")?;
            (&braced[..end], &braced[end + 1..])
        } else {
            let end = reference.find(['/', '\\']).unwrap_or(reference.len());
            (&reference[..end], &reference[end..])
        };
        if selector.is_empty() || (!suffix.is_empty() && !suffix.starts_with(['/', '\\'])) {
            return Err("Invalid workspace reference. Use @name/path or @{root-id}/path.".into());
        }
        // Stable IDs disambiguate roots whose display names are identical.
        let by_id: Vec<_> = self.roots.iter().filter(|root| root.id == selector).collect();
        let matches: Vec<_> = if by_id.is_empty() {
            self.roots
                .iter()
                .filter(|root| {
                    if cfg!(windows) {
                        root.name.to_lowercase() == selector.to_lowercase()
                    } else {
                        root.name == selector
                    }
                })
                .collect()
        } else {
            by_id
        };
        let root = match matches.as_slice() {
            [] => {
                return Err(format!(
                    "Unknown workspace root '{selector}'. Use Workspace to inspect current roots; ask the user if the intended root is unclear."
                ));
            }
            [root] => *root,
            _ => {
                return Err(format!(
                    "Workspace root '{selector}' is ambiguous. Use Workspace to inspect root IDs and ask the user which root is intended; do not guess."
                ));
            }
        };
        if !root.available || !root.path.is_absolute() || !root.path.is_dir() {
            return Err(format!(
                "Workspace root '{selector}' is unavailable. Ask the user to relocate it; no fallback directory was selected."
            ));
        }
        let mut target = root.path.clone();
        let mut depth = 0usize;
        // Both slash forms are reference separators, independent of the host platform.
        for part in suffix.trim_start_matches(['/', '\\']).split(['/', '\\']) {
            match part {
                "" | "." => {},
                ".." if depth > 0 => { target.pop(); depth -= 1; },
                ".." => return Err("Workspace reference points outside its named root. Use an explicit absolute path for a deliberate outside-root operation.".into()),
                _ => {
                    if part.contains(':') || !Path::new(part).components().all(|c| matches!(c, Component::Normal(_))) {
                        return Err("Invalid workspace-relative path component".into());
                    }
                    target.push(part);
                    depth += 1;
                }
            }
        }
        let canonical_root = root
            .path
            .canonicalize()
            .map_err(|e| format!("Workspace root unavailable: {e}"))?;
        // Validate the nearest existing ancestor as Write may create a new file/directory.
        let mut ancestor = target.as_path();
        loop {
            match ancestor.symlink_metadata() {
                Ok(_) => {
                    let canonical = ancestor
                        .canonicalize()
                        .map_err(|e| format!("Invalid workspace reference target: {e}"))?;
                    if !canonical.starts_with(&canonical_root) {
                        return Err("Workspace reference resolves outside its named root through a link. Use an explicit absolute path.".into());
                    }
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    ancestor = ancestor.parent().ok_or("Workspace reference has no valid ancestor")?;
                }
                Err(e) => return Err(format!("Cannot inspect workspace reference: {e}")),
            }
        }
        Ok(Some(target))
    }
}
