use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn run(root: &Path, pattern: &str, glob: Option<&str>, case_insensitive: bool) -> SearchOutcome {
        search(&SearchRequest {
            pattern,
            root,
            glob,
            case_insensitive,
        })
        .unwrap()
    }

    fn file_names(outcome: &SearchOutcome) -> Vec<String> {
        outcome
            .lines
            .iter()
            .map(|line| {
                let path = line.rsplitn(3, ':').nth(2).unwrap();
                Path::new(path).file_name().unwrap().to_string_lossy().into_owned()
            })
            .collect()
    }

    #[test]
    fn alternation_and_quantifiers_match_across_files() {
        // findstr, the old Windows fallback, matched none of these.
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.c"), "void meter_tick(void);\n").unwrap();
        fs::write(dir.path().join("b.c"), "uint16_t g_regs[8];\nint   spaced;\n").unwrap();
        let outcome = run(dir.path(), r"meter_tick|g_regs|int\s+spaced", None, false);
        assert_eq!(outcome.lines.len(), 3, "{:?}", outcome.lines);
        assert!(outcome.lines[0].ends_with(":1:void meter_tick(void);"));
        assert!(outcome.lines[2].ends_with(":2:int   spaced;"));
    }

    #[test]
    fn glob_whitelists_file_names_at_any_depth() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        fs::write(dir.path().join("src/deep/x.c"), "needle\n").unwrap();
        fs::write(dir.path().join("src/deep/x.h"), "needle\n").unwrap();
        fs::write(dir.path().join("notes.txt"), "needle\n").unwrap();
        let outcome = run(dir.path(), "needle", Some("*.{c,h}"), false);
        assert_eq!(file_names(&outcome), ["x.c", "x.h"]);
    }

    #[test]
    fn gitignored_hidden_and_binary_files_are_skipped() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "build/\n").unwrap();
        fs::create_dir_all(dir.path().join("build/bin")).unwrap();
        fs::create_dir_all(dir.path().join(".cache")).unwrap();
        fs::write(dir.path().join("build/bin/out.txt"), "needle\n").unwrap();
        fs::write(dir.path().join(".cache/x.txt"), "needle\n").unwrap();
        fs::write(dir.path().join("app.exe"), b"MZ\0\0needle\0").unwrap();
        fs::write(dir.path().join("main.c"), "needle\n").unwrap();
        let outcome = run(dir.path(), "needle", None, false);
        assert_eq!(file_names(&outcome), ["main.c"]);
        assert_eq!(outcome.skipped_binary, 1);
    }

    #[test]
    fn case_insensitive_search_is_opt_in() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "Needle\n").unwrap();
        assert!(run(dir.path(), "needle", None, false).lines.is_empty());
        assert_eq!(run(dir.path(), "needle", None, true).lines.len(), 1);
    }

    #[test]
    fn non_utf8_text_still_matches_ascii_identifiers() {
        // GBK comment bytes around an identifier, as in Chinese embedded C sources.
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("gbk.c"), b"/* \xB5\xE7\xB1\xED */ int meter_tick;\r\n").unwrap();
        let outcome = run(dir.path(), r"int meter_tick;$", None, false);
        assert_eq!(outcome.lines.len(), 1);
        assert!(outcome.lines[0].ends_with("int meter_tick;"), "{}", outcome.lines[0]);
    }

    #[test]
    fn utf16_with_bom_is_searched_as_text() {
        let dir = tempdir().unwrap();
        let mut raw = vec![0xFF, 0xFE];
        for unit in "first\r\nsecond needle\r\n".encode_utf16() {
            raw.extend_from_slice(&unit.to_le_bytes());
        }
        fs::write(dir.path().join("wide.rc"), raw).unwrap();
        let outcome = run(dir.path(), "needle", None, false);
        assert_eq!(outcome.skipped_binary, 0);
        assert_eq!(outcome.lines.len(), 1);
        assert!(outcome.lines[0].ends_with(":2:second needle"));
    }

    #[test]
    fn line_anchors_ignore_a_utf8_bom_and_the_final_newline() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.c"), "\u{FEFF}#include <x.h>\n").unwrap();
        let outcome = run(dir.path(), "^#include", None, false);
        assert_eq!(outcome.lines.len(), 1);
        // A pattern that matches empty lines must not report a phantom line after the last newline.
        assert_eq!(run(dir.path(), "^$", None, false).lines.len(), 0);
    }

    #[test]
    fn results_stop_at_the_line_limit() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("many.txt"), "hit\n".repeat(MAX_RESULT_LINES + 5)).unwrap();
        let outcome = run(dir.path(), "hit", None, false);
        assert_eq!(outcome.lines.len(), MAX_RESULT_LINES);
        assert!(outcome.truncated);
    }

    #[test]
    fn a_single_file_root_is_searched() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("only.txt");
        fs::write(&file, "one\nneedle\n").unwrap();
        let outcome = run(&file, "needle", None, false);
        assert_eq!(outcome.lines.len(), 1);
        assert!(outcome.lines[0].ends_with(":2:needle"));
    }

    #[test]
    fn very_long_lines_are_cut() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("min.js"), format!("needle{}\n", "x".repeat(2_000))).unwrap();
        let outcome = run(dir.path(), "needle", None, false);
        let text = outcome.lines[0].rsplit(':').next().unwrap();
        assert_eq!(text.chars().count(), MAX_LINE_CHARS + 1);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn invalid_patterns_and_missing_paths_are_errors() {
        let dir = tempdir().unwrap();
        let bad_regex = search(&SearchRequest {
            pattern: "(unclosed",
            root: dir.path(),
            glob: None,
            case_insensitive: false,
        });
        assert!(bad_regex.unwrap_err().starts_with("Invalid regex pattern"));
        let missing = search(&SearchRequest {
            pattern: "x",
            root: &dir.path().join("absent"),
            glob: None,
            case_insensitive: false,
        });
        assert!(missing.unwrap_err().starts_with("Path does not exist"));
        let bad_glob = search(&SearchRequest {
            pattern: "x",
            root: dir.path(),
            glob: Some("{unclosed"),
            case_insensitive: false,
        });
        assert!(bad_glob.unwrap_err().starts_with("Invalid glob pattern"));
    }
}
