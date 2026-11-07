//! In-process content search behind the Grep tool.
//!
//! The search runs inside the agent process instead of spawning `rg` or a
//! platform tool. Without ripgrep on PATH the old code fell back to `findstr`
//! on Windows, which has no alternation, `+` or groups, so most patterns
//! silently returned no matches. Running in-process also means a host that
//! only trusts its own binary to read plain file contents (transparent file
//! encryption) gives Grep the same bytes the Read tool sees.

use std::fs;
use std::path::Path;

use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use regex::bytes::{Regex, RegexBuilder};

/// Matching lines kept in one result, as promised by the tool description.
pub(crate) const MAX_RESULT_LINES: usize = 250;
/// Leading bytes checked for a NUL byte when deciding whether a file is binary.
const BINARY_PROBE_BYTES: usize = 8 * 1024;
/// Larger files are skipped instead of being loaded into memory.
pub(crate) const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// Longest line text kept in one result row, in characters.
const MAX_LINE_CHARS: usize = 500;

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

pub(crate) struct SearchRequest<'a> {
    pub(crate) pattern: &'a str,
    pub(crate) root: &'a Path,
    pub(crate) glob: Option<&'a str>,
    pub(crate) case_insensitive: bool,
}

#[derive(Debug, Default)]
pub(crate) struct SearchOutcome {
    /// `path:line:text` rows in path order.
    pub(crate) lines: Vec<String>,
    /// More lines matched than `MAX_RESULT_LINES`.
    pub(crate) truncated: bool,
    pub(crate) skipped_binary: usize,
    pub(crate) skipped_large: usize,
    /// Files or directories that could not be listed or read.
    pub(crate) unreadable: usize,
}

/// Search every non-ignored file under `request.root` (or the file itself).
///
/// Walking follows ripgrep's defaults: `.gitignore` and `.ignore` files are
/// honoured, hidden entries are skipped, and an optional glob whitelists file
/// names the way `rg --glob` does. `.gitignore` also applies outside a Git
/// repository, since agent workspaces are not always repositories.
pub(crate) fn search(request: &SearchRequest<'_>) -> Result<SearchOutcome, String> {
    let regex = RegexBuilder::new(request.pattern)
        .case_insensitive(request.case_insensitive)
        .build()
        .map_err(|error| format!("Invalid regex pattern: {error}"))?;
    if !request.root.exists() {
        return Err(format!("Path does not exist: {}", request.root.display()));
    }

    let mut walker = WalkBuilder::new(request.root);
    walker.require_git(false).sort_by_file_name(|a, b| a.cmp(b));
    if let Some(glob) = request.glob.map(str::trim).filter(|glob| !glob.is_empty()) {
        let mut overrides = OverrideBuilder::new(request.root);
        overrides
            .add(glob)
            .map_err(|error| format!("Invalid glob pattern: {error}"))?;
        let overrides = overrides
            .build()
            .map_err(|error| format!("Invalid glob pattern: {error}"))?;
        walker.overrides(overrides);
    }

    let mut outcome = SearchOutcome::default();
    for entry in walker.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::debug!(%error, "Grep could not list an entry");
                outcome.unreadable += 1;
                continue;
            }
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        search_file(&regex, entry.path(), &mut outcome);
        if outcome.truncated {
            break;
        }
    }
    Ok(outcome)
}

fn search_file(regex: &Regex, path: &Path, outcome: &mut SearchOutcome) {
    match fs::metadata(path) {
        Ok(metadata) if metadata.len() > MAX_FILE_BYTES => {
            outcome.skipped_large += 1;
            return;
        }
        Ok(_) => {}
        Err(error) => {
            tracing::debug!(%error, path = %path.display(), "Grep could not stat a file");
            outcome.unreadable += 1;
            return;
        }
    }
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::debug!(%error, path = %path.display(), "Grep could not read a file");
            outcome.unreadable += 1;
            return;
        }
    };
    let decoded = decode_utf16_with_bom(&raw);
    let text = decoded.as_deref().unwrap_or(&raw);
    if decoded.is_none() && text[..text.len().min(BINARY_PROBE_BYTES)].contains(&0) {
        outcome.skipped_binary += 1;
        return;
    }
    let text = text.strip_prefix(UTF8_BOM).unwrap_or(text);
    let text = text.strip_suffix(b"\n").unwrap_or(text);
    for (index, line) in text.split(|byte| *byte == b'\n').enumerate() {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if !regex.is_match(line) {
            continue;
        }
        if outcome.lines.len() == MAX_RESULT_LINES {
            outcome.truncated = true;
            return;
        }
        outcome
            .lines
            .push(format!("{}:{}:{}", path.display(), index + 1, display_line(line)));
    }
}

/// Transcode a UTF-16 file that starts with a byte order mark into UTF-8, as
/// ripgrep does. Its NUL bytes would otherwise mark it as binary.
fn decode_utf16_with_bom(raw: &[u8]) -> Option<Vec<u8>> {
    let (body, little_endian) = match raw {
        [0xFF, 0xFE, body @ ..] => (body, true),
        [0xFE, 0xFF, body @ ..] => (body, false),
        _ => return None,
    };
    let units = body.chunks_exact(2).map(|pair| {
        if little_endian {
            u16::from_le_bytes([pair[0], pair[1]])
        } else {
            u16::from_be_bytes([pair[0], pair[1]])
        }
    });
    Some(
        char::decode_utf16(units)
            .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect::<String>()
            .into_bytes(),
    )
}

/// Show a matched line as text, replacing invalid UTF-8 and cutting very long
/// lines such as minified files.
fn display_line(line: &[u8]) -> String {
    let text = String::from_utf8_lossy(line);
    match text.char_indices().nth(MAX_LINE_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.into_owned(),
    }
}

#[cfg(test)]
#[path = "grep_search_test.rs"]
mod grep_search_test;
