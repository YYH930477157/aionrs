use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn grep_tool_finds_pattern_in_own_source() {
        let tool = GrepTool::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let input = json!({
            "pattern": "GrepTool",
            "path": env!("CARGO_MANIFEST_DIR")
        });
        let result = tool.execute(input).await;
        assert!(!result.is_error, "grep failed: {}", result.content);
        assert!(result.content.contains("GrepTool"));
    }

    #[tokio::test]
    async fn execute_uses_cwd_for_relative_path() {
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("searchable.txt"), "unique_grep_marker_xyz").unwrap();

        let tool = GrepTool::new(tmp.path().to_path_buf());
        let input = json!({"pattern": "unique_grep_marker_xyz", "path": "."});
        let result = tool.execute(input).await;
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            result.content.contains("unique_grep_marker_xyz"),
            "should find pattern, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn an_empty_result_says_which_files_were_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("firmware.bin"), b"\0\0needle").unwrap();
        let tool = GrepTool::new(tmp.path().to_path_buf());
        let result = tool.execute(json!({"pattern": "needle"})).await;
        assert!(!result.is_error);
        assert_eq!(result.content, "No matches found\n[1 binary files were skipped]");
    }

    #[tokio::test]
    async fn truncated_output_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("many.txt"), "hit\n".repeat(300)).unwrap();
        let tool = GrepTool::new(tmp.path().to_path_buf());
        let result = tool.execute(json!({"pattern": "hit"})).await;
        assert_eq!(result.content.lines().count(), 251);
        assert!(
            result
                .content
                .ends_with("[Output truncated to the first 250 matching lines]")
        );
    }

    #[tokio::test]
    async fn an_invalid_regex_is_an_error_not_an_empty_result() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = GrepTool::new(tmp.path().to_path_buf());
        let result = tool.execute(json!({"pattern": "(unclosed"})).await;
        assert!(result.is_error);
        assert!(
            result.content.starts_with("Invalid regex pattern"),
            "{}",
            result.content
        );
    }
}
