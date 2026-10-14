use crate::CapabilityRegistry;
use artex_core::{Error, Result};
use globset::{GlobBuilder, GlobMatcher};
use regex::RegexBuilder;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};
fn matcher(pattern: &str) -> Result<GlobMatcher> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .backslash_escape(false)
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|_| Error::new("INVALID_PARAMS", "Invalid glob pattern"))
}
fn walk(registry: &CapabilityRegistry, cap: &str, path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.into()]);
    }
    let root = registry.root(cap)?;
    let mut files = vec![];
    let start = Instant::now();
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            !e.file_type().is_dir()
                || !matches!(
                    e.file_name().to_str(),
                    Some(".git" | "node_modules" | "vendor" | ".artex-host")
                )
        })
    {
        if start.elapsed().as_secs() > 30 || files.len() >= 100000 {
            return Err(Error::new("SIZE_LIMIT", "Search traversal budget exceeded"));
        }
        let entry =
            entry.map_err(|_| Error::new("IO_FAILURE", "Cannot traverse search directory"))?;
        if entry.file_type().is_symlink() {
            continue;
        }
        if entry.file_type().is_file() {
            let relative = entry
                .path()
                .strip_prefix(&root)
                .map_err(|_| Error::new("PERMISSION_DENIED", "Search escaped capability"))?;
            files.push(registry.resolve(cap, &relative.to_string_lossy(), false)?);
        }
    }
    files.sort();
    Ok(files)
}
pub fn execute(registry: &CapabilityRegistry, name: &str, input: &Value) -> Result<Value> {
    let cap = input["directory_capability"]
        .as_str()
        .ok_or_else(|| Error::new("PERMISSION_DENIED", "Search capability required"))?;
    let path = registry.resolve(cap, input["path"].as_str().unwrap_or("."), false)?;
    let files = walk(registry, cap, &path)?;
    let base = if path.is_file() {
        path.parent().unwrap()
    } else {
        &path
    };
    let pattern = input["pattern"]
        .as_str()
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Search pattern required"))?;
    if name == "Glob" {
        let glob = matcher(pattern)?;
        let mut matches = vec![];
        for file in files {
            let relative = file.strip_prefix(base).unwrap();
            if glob.is_match(relative)
                || !pattern.contains('/') && glob.is_match(file.file_name().unwrap())
            {
                let modified = fs::metadata(&file).and_then(|m| m.modified()).ok();
                matches.push((modified, relative.to_string_lossy().replace('\\', "/")));
            }
        }
        matches.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        return Ok(json!({"paths":matches.iter().map(|m|&m.1).collect::<Vec<_>>()}));
    }
    let regex = RegexBuilder::new(pattern)
        .case_insensitive(input["-i"].as_bool().unwrap_or(false))
        .dot_matches_new_line(input["multiline"].as_bool().unwrap_or(false))
        .size_limit(8 * 1024 * 1024)
        .build()
        .map_err(|_| Error::new("INVALID_PARAMS", "Invalid regular expression"))?;
    let glob = input["glob"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(matcher)
        .transpose()?;
    let typ = input["type"].as_str().filter(|s| !s.is_empty());
    let mode = input["output_mode"]
        .as_str()
        .unwrap_or("files_with_matches");
    if !matches!(mode, "files_with_matches" | "content" | "count") {
        return Err(Error::new("INVALID_PARAMS", "Unknown grep output mode"));
    }
    let before = input["-C"]
        .as_u64()
        .unwrap_or(0)
        .max(input["-B"].as_u64().unwrap_or(0))
        .min(1000) as usize;
    let after = input["-C"]
        .as_u64()
        .unwrap_or(0)
        .max(input["-A"].as_u64().unwrap_or(0))
        .min(1000) as usize;
    let mut results = vec![];
    let mut scanned = 0u64;
    for file in files {
        let relative = file
            .strip_prefix(base)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if glob
            .as_ref()
            .is_some_and(|g| !g.is_match(&relative) && !g.is_match(file.file_name().unwrap()))
        {
            continue;
        }
        if typ.is_some_and(|t| !type_matches(t, &file)) {
            continue;
        }
        let size = fs::metadata(&file).map_err(Error::io)?.len();
        if size > 16 * 1024 * 1024 {
            continue;
        }
        scanned += size;
        if scanned > 256 * 1024 * 1024 {
            return Err(Error::new("SIZE_LIMIT", "Grep scan byte budget exceeded"));
        }
        let bytes = fs::read(&file).map_err(Error::io)?;
        if bytes.contains(&0) {
            continue;
        }
        let content = match String::from_utf8(bytes) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let lines: Vec<_> = content.lines().collect();
        let mut hit_lines = std::collections::BTreeSet::new();
        if input["multiline"].as_bool().unwrap_or(false) {
            for hit in regex.find_iter(&content) {
                let start = content[..hit.start()]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count();
                let end = content[..hit.end()].bytes().filter(|b| *b == b'\n').count();
                for i in start..=end {
                    hit_lines.insert(i);
                }
            }
        } else {
            for (i, line) in lines.iter().enumerate() {
                if regex.is_match(line) {
                    hit_lines.insert(i);
                }
            }
        }
        if hit_lines.is_empty() {
            continue;
        }
        match mode {
            "files_with_matches" => results.push(relative),
            "count" => results.push(format!("{relative}: {}", hit_lines.len())),
            _ => {
                let mut emitted = std::collections::BTreeSet::new();
                for i in hit_lines {
                    for j in i.saturating_sub(before)
                        ..=i.saturating_add(after).min(lines.len().saturating_sub(1))
                    {
                        if emitted.insert(j) {
                            results.push(if input["-n"].as_bool().unwrap_or(true) {
                                format!("{relative}:{}:{}", j + 1, lines[j])
                            } else {
                                format!("{relative}:{}", lines[j])
                            });
                        }
                    }
                }
            }
        }
        if results.iter().map(String::len).sum::<usize>() > 8 * 1024 * 1024 {
            return Err(Error::new("SIZE_LIMIT", "Grep result budget exceeded"));
        }
    }
    let offset = input["offset"].as_u64().unwrap_or(0) as usize;
    let limit = input["head_limit"].as_u64().unwrap_or(250) as usize;
    let results: Vec<_> = results
        .into_iter()
        .skip(offset)
        .take(if limit == 0 { usize::MAX } else { limit })
        .collect();
    Ok(json!({"output":results.join("\n"),"results":results,"output_mode":mode}))
}
fn type_matches(kind: &str, path: &Path) -> bool {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    match kind {
        "rust" => ext == "rs",
        "js" => matches!(ext, "js" | "jsx" | "mjs" | "cjs"),
        "ts" => matches!(ext, "ts" | "tsx"),
        "py" => ext == "py",
        "cpp" => matches!(ext, "cpp" | "hpp" | "cc" | "h"),
        _ => ext == kind,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_filters_binary_and_supports_context() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("src")).unwrap();
        fs::write(d.path().join("src/a.rs"), "first\nHELLO\nlast\n").unwrap();
        fs::write(d.path().join("binary.rs"), b"HELLO\0").unwrap();
        let r = CapabilityRegistry::default();
        let cap = r.grant_directory(d.path()).unwrap();
        let g = execute(
            &r,
            "Glob",
            &json!({"directory_capability":cap,"pattern":"**/*.rs"}),
        )
        .unwrap();
        assert_eq!(g["paths"].as_array().unwrap().len(), 2);
        let v=execute(&r,"Grep",&json!({"directory_capability":cap,"pattern":"hello","-i":true,"-C":1,"output_mode":"content"})).unwrap();
        assert!(v["output"].as_str().unwrap().contains("src/a.rs:1:first"));
        assert!(!v["output"].as_str().unwrap().contains("binary"));
    }
}
