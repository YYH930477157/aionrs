use std::path::{Component, Path, PathBuf};

use super::WorkspaceContext;

impl WorkspaceContext {
    fn names_existing_path(&self, value: &str) -> bool {
        self.default_cwd
            .as_ref()
            .is_some_and(|default| default.join(value).exists())
    }

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
            // `@scope/package` is also an ordinary folder name, so a path that exists stays a path.
            [] if self.roots.is_empty() || self.names_existing_path(value) => return Ok(None),
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

#[cfg(test)]
mod tests {
    use super::super::WorkspaceRoot;
    use super::*;

    fn with_root(default: &Path, root: &Path) -> WorkspaceContext {
        WorkspaceContext {
            default_cwd: Some(default.to_path_buf()),
            roots: vec![WorkspaceRoot {
                id: "lib".into(),
                name: "library".into(),
                path: root.to_path_buf(),
                available: true,
            }],
        }
    }

    #[test]
    fn a_single_folder_conversation_keeps_at_paths_as_ordinary_paths() {
        let context = WorkspaceContext::legacy(PathBuf::from("/project"));

        assert_eq!(context.resolve_reference("@types/node/index.d.ts"), Ok(None));
    }

    #[test]
    fn an_existing_scoped_folder_is_not_mistaken_for_a_root() {
        let default = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(default.path().join("@scope")).unwrap();

        let context = with_root(default.path(), root.path());

        assert_eq!(context.resolve_reference("@scope"), Ok(None));
    }

    #[test]
    fn a_mistyped_root_is_still_reported() {
        let default = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();

        let error = with_root(default.path(), root.path())
            .resolve_reference("@librarry/file.txt")
            .unwrap_err();

        assert!(error.contains("Unknown workspace root"), "{error}");
    }

    #[test]
    fn a_named_root_wins_over_a_folder_with_the_same_name() {
        let default = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(default.path().join("@library")).unwrap();

        let resolved = with_root(default.path(), root.path())
            .resolve_reference("@library/file.txt")
            .unwrap();

        assert_eq!(resolved, Some(root.path().join("file.txt")));
    }
}
