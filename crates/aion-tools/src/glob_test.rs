use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    use aion_types::tool::ToolResult;

    async fn run_glob(pattern: &str, path: &str) -> ToolResult {
        let tool = GlobTool::new(PathBuf::from(path));
        let input = json!({ "pattern": pattern, "path": path });
        tool.execute(input).await
    }

    #[tokio::test]
    async fn test_glob_matches_pattern() {
        let dir = tempdir().unwrap();
        let base = dir.path();

        fs::write(base.join("main.rs"), "fn main() {}").unwrap();
        fs::write(base.join("lib.rs"), "pub mod lib;").unwrap();
        fs::write(base.join("notes.txt"), "some notes").unwrap();
        fs::write(base.join("readme.md"), "# Readme").unwrap();

        let result = run_glob("*.rs", base.to_str().unwrap()).await;

        assert!(!result.is_error, "glob should succeed");
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 2, "should match exactly 2 .rs files");
        for line in &lines {
            assert!(line.ends_with(".rs"), "each match should be a .rs file, got: {}", line);
        }
        assert!(!result.content.contains("notes.txt"), "should not include .txt files");
        assert!(!result.content.contains("readme.md"), "should not include .md files");
    }

    #[tokio::test]
    async fn test_glob_no_matches() {
        let dir = tempdir().unwrap();
        let base = dir.path();

        fs::write(base.join("main.rs"), "fn main() {}").unwrap();
        fs::write(base.join("lib.rs"), "pub mod lib;").unwrap();

        let result = run_glob("*.xyz", base.to_str().unwrap()).await;

        assert!(!result.is_error, "no-match glob should not be an error");
        assert_eq!(result.content, "No files matched the pattern");
    }

    #[tokio::test]
    async fn test_glob_with_limit() {
        let dir = tempdir().unwrap();
        let base = dir.path();

        for i in 0..5 {
            fs::write(base.join(format!("file_{}.txt", i)), format!("content {}", i)).unwrap();
        }

        let result = run_glob("*.txt", base.to_str().unwrap()).await;

        assert!(!result.is_error, "glob should succeed");
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 5, "all 5 files should be returned");
    }

    #[tokio::test]
    async fn test_glob_recursive() {
        let dir = tempdir().unwrap();
        let base = dir.path();

        // Create nested directory structure
        let sub_a = base.join("a");
        let sub_b = base.join("a").join("b");
        fs::create_dir_all(&sub_b).unwrap();

        fs::write(base.join("root.txt"), "root level").unwrap();
        fs::write(sub_a.join("mid.txt"), "middle level").unwrap();
        fs::write(sub_b.join("deep.txt"), "deep level").unwrap();
        // Non-matching file
        fs::write(sub_a.join("skip.rs"), "not a txt").unwrap();

        let result = run_glob("**/*.txt", base.to_str().unwrap()).await;

        assert!(!result.is_error, "recursive glob should succeed");
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 3, "should find 3 .txt files across all levels");
        assert!(result.content.contains("root.txt"), "should include root-level file");
        assert!(result.content.contains("mid.txt"), "should include mid-level file");
        assert!(result.content.contains("deep.txt"), "should include deep-level file");
        assert!(!result.content.contains("skip.rs"), "should not include .rs files");
    }

    #[tokio::test]
    async fn brace_alternatives_match_each_extension_under_a_non_ascii_folder() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("需求");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("list.xlsx"), "x").unwrap();
        fs::write(folder.join("list.csv"), "c").unwrap();
        fs::write(folder.join("notes.txt"), "t").unwrap();

        let result = run_glob("**/*.{xlsx,xls,csv}", dir.path().to_str().unwrap()).await;

        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("list.xlsx"), "got: {}", result.content);
        assert!(result.content.contains("list.csv"), "got: {}", result.content);
        assert!(!result.content.contains("notes.txt"), "got: {}", result.content);
    }

    #[tokio::test]
    async fn the_newest_files_survive_the_limit_and_the_cut_is_reported() {
        let dir = tempdir().unwrap();
        let early = dir.path().join("a");
        let late = dir.path().join("z");
        fs::create_dir_all(&early).unwrap();
        fs::create_dir_all(&late).unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        for i in 0..MAX_RESULTS + 1 {
            let path = early.join(format!("old_{i:03}.txt"));
            fs::write(&path, "old").unwrap();
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        // Walked last, but the newest: it must head the list.
        fs::write(late.join("newest.txt"), "new").unwrap();

        let result = run_glob("**/*.txt", dir.path().to_str().unwrap()).await;

        assert!(!result.is_error, "{}", result.content);
        assert!(
            result.content.lines().next().unwrap().ends_with("newest.txt"),
            "got: {}",
            result.content
        );
        assert!(
            result
                .content
                .contains("(Showing the 100 newest of 102 matching files."),
            "got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn a_recursive_search_above_the_workspace_is_refused_but_a_sibling_is_not() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().join("project");
        let sibling = dir.path().join("notes");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        fs::write(dir.path().join("answer.txt"), "secret").unwrap();
        fs::write(sibling.join("plan.txt"), "plan").unwrap();
        let tool = GlobTool::new(workspace.clone());

        let above = tool
            .execute(json!({"pattern": "**/*.txt", "path": dir.path().to_str().unwrap()}))
            .await;
        assert!(above.is_error, "got: {}", above.content);
        assert!(above.content.starts_with("Refused:"), "got: {}", above.content);
        assert!(!above.content.contains("answer.txt"));

        let absolute = format!("{}/**/*.txt", dir.path().display());
        let absolute = tool.execute(json!({"pattern": absolute})).await;
        assert!(absolute.is_error, "got: {}", absolute.content);

        let one_level = tool
            .execute(json!({"pattern": "*.txt", "path": dir.path().to_str().unwrap()}))
            .await;
        assert!(!one_level.is_error, "got: {}", one_level.content);

        let beside = tool
            .execute(json!({"pattern": "**/*.txt", "path": sibling.to_str().unwrap()}))
            .await;
        assert!(!beside.is_error, "got: {}", beside.content);
        assert!(beside.content.contains("plan.txt"));
    }

    #[test]
    fn braces_expand_nested_and_repeated_groups_and_leave_single_items_alone() {
        assert_eq!(expand_braces("x{a,{b,c}}y"), vec!["xay", "xby", "xcy"]);
        assert_eq!(
            expand_braces("s/*.{c,h}/{p,q}"),
            vec!["s/*.c/p", "s/*.c/q", "s/*.h/p", "s/*.h/q"]
        );
        assert_eq!(expand_braces("only{one}.rs"), vec!["only{one}.rs"]);
        assert_eq!(expand_braces("open{a,b"), vec!["open{a,b"]);
        assert_eq!(expand_braces("**/*.rs"), vec!["**/*.rs"]);
    }

    #[tokio::test]
    async fn execute_uses_cwd_for_relative_path() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("marker.txt"), "hello").unwrap();

        let tool = GlobTool::new(tmp.path().to_path_buf());
        let input = json!({"pattern": "marker.txt"});
        let result = tool.execute(input).await;
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            result.content.contains("marker.txt"),
            "should find marker.txt, got: {}",
            result.content
        );
    }
}
