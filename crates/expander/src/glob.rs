//! Pathname (filename) expansion. Splits the pattern on `/`, walks the
//! filesystem per segment, and matches against `pattern::fnmatch`.
//!
//! Honors `shopt` flags via `Environment::option`: `dotglob`, `nullglob`,
//! `failglob`, `nocaseglob`, `globstar`, `extglob`, `globskipdots`,
//! `globignore`.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use cherubsh_common::Environment;

use crate::buf::dequote_bytes;
use crate::error::ExpandError;
use crate::pattern::{
    explicitly_matches_dot_name, explicitly_matches_dot_name_parsed, fnmatch, fnmatch_parsed,
    has_glob_meta, parse, GlobOpts, Tok,
};
use crate::wd::Wd;

#[derive(Clone, Debug, Default)]
pub struct GlobFlags {
    pub opts: GlobOpts,
    pub dotglob: bool,
    pub nullglob: bool,
    pub failglob: bool,
    pub globstar: bool,
    pub globskipdots: bool,
    pub globignore: Vec<Vec<u8>>,
    pub globsort: GlobSort,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GlobSort {
    #[default]
    NameAsc,
    NameDesc,
    NoSort,
    AtimeAsc,
    AtimeDesc,
    MtimeAsc,
    MtimeDesc,
    SizeAsc,
    SizeDesc,
}

impl GlobFlags {
    pub fn from_env(env: &dyn Environment) -> Self {
        let nocaseglob = env.option("nocaseglob");
        let extglob = env.option("extglob");
        let mut dotglob = env.option("dotglob");
        let nullglob = env.option("nullglob");
        let failglob = env.option("failglob");
        let globstar = env.option("globstar");
        let globskipdots = env.option("globskipdots");
        let mut globignore = Vec::new();
        if let Some(v) = env.get("GLOBIGNORE") {
            for piece in split_globignore(&v) {
                if !piece.is_empty() {
                    globignore.push(piece.to_vec());
                }
            }
        }
        if !globignore.is_empty() {
            dotglob = true;
        }
        let globsort = env
            .get("GLOBSORT")
            .as_deref()
            .map(parse_globsort)
            .unwrap_or_default();
        Self {
            opts: GlobOpts {
                nocaseglob,
                extglob,
                globasciiranges: false,
            },
            dotglob,
            nullglob,
            failglob,
            globstar,
            globskipdots,
            globignore,
            globsort,
        }
    }
}

/// Expand the pattern stored in `wd` into a list of `Wd` entries (one per
/// matching file). Returns the input itself when no glob meta characters are
/// present (the caller may still keep the word as-is).
pub fn pathname_expand(wd: &Wd, env: &dyn Environment) -> Result<Vec<Wd>, ExpandError> {
    let flags = GlobFlags::from_env(env);
    if !has_glob_meta(wd.buf.as_bytes(), flags.opts) {
        return Ok(vec![wd.clone()]);
    }
    let absolute = wd.buf.as_bytes().first() == Some(&b'/');
    let mut segments: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut i = 0;
    let bytes = wd.buf.as_bytes();
    while i < bytes.len() {
        let b = bytes[i];
        if b == crate::buf::CTLESC && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                segments.push(std::mem::take(&mut cur));
                i += 2;
                continue;
            }
            cur.push(crate::buf::CTLESC);
            cur.push(bytes[i + 1]);
            i += 2;
            continue;
        }
        if b == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            if has_glob_meta(&cur, flags.opts) {
                cur.push(b'\\');
                cur.push(b'/');
            } else {
                segments.push(std::mem::take(&mut cur));
            }
            i += 2;
            continue;
        }
        if b == b'/' {
            segments.push(std::mem::take(&mut cur));
            i += 1;
            continue;
        }
        cur.push(b);
        i += 1;
    }
    segments.push(cur);
    let mut collapsed_adjacent_globstar = false;
    if flags.globstar {
        let mut collapsed: Vec<Vec<u8>> = Vec::with_capacity(segments.len());
        for segment in segments {
            if segment == b"**"
                && collapsed
                    .last()
                    .is_some_and(|previous| previous.as_slice() == b"**")
            {
                collapsed_adjacent_globstar = true;
                continue;
            }
            collapsed.push(segment);
        }
        segments = collapsed;
    }
    let trailing_slash = bytes.last() == Some(&b'/');
    let globstar_segment_count = if flags.globstar {
        segments
            .iter()
            .filter(|segment| segment.as_slice() == b"**")
            .count()
    } else {
        0
    };
    let force_zero_depth_globstar_slash = flags.globstar
        && !collapsed_adjacent_globstar
        && globstar_segment_count == 1
        && segments.last().is_some_and(|segment| segment == b"**")
        && segments.len() >= 2
        && !segments[segments.len() - 2].is_empty()
        && !has_glob_meta(&segments[segments.len() - 2], flags.opts);

    let mut current_dirs: Vec<PathBuf> = if absolute {
        vec![PathBuf::from("/")]
    } else {
        vec![PathBuf::from(".")]
    };
    let start_index = if absolute { 1 } else { 0 };

    for (idx, segment) in segments.iter().enumerate().skip(start_index) {
        let is_last = idx == segments.len() - 1;
        if segment.is_empty() {
            // Trailing slash - keep dirs as-is but only if they are directories.
            let mut kept = Vec::new();
            for d in current_dirs {
                if d.is_dir() && d.as_os_str().as_encoded_bytes() != b"." {
                    kept.push(d);
                }
            }
            current_dirs = kept;
            continue;
        }
        if !has_glob_meta(segment, flags.opts) {
            // Literal segment.
            let segment_dequoted = dequote_glob_literal_segment(segment);
            let mut next = Vec::new();
            for d in &current_dirs {
                let mut candidate = d.clone();
                candidate.push(std::ffi::OsStr::from_bytes(&segment_dequoted));
                if candidate.exists() {
                    next.push(candidate);
                }
            }
            current_dirs = next;
            continue;
        }
        // Wildcard segment.
        let mut next = Vec::new();
        let segment_tokens = parse(segment, flags.opts);
        if flags.globstar && segment == b"**" {
            for d in &current_dirs {
                if is_last {
                    let include_start = absolute || idx != start_index;
                    collect_recursive_all(
                        d,
                        &mut next,
                        &flags,
                        include_start,
                        force_zero_depth_globstar_slash,
                    );
                } else {
                    let include_symlink_dirs =
                        idx + 1 == segments.len() - 1 && segments[idx + 1].is_empty();
                    collect_recursive_dirs(d, &mut next, &flags, include_symlink_dirs);
                }
            }
            current_dirs = next;
            continue;
        }
        for d in &current_dirs {
            let entries = match std::fs::read_dir(d) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            let mut names: Vec<(Vec<u8>, PathBuf)> = Vec::new();
            if !flags.globskipdots {
                let mut dot = d.clone();
                dot.push(".");
                if matches_glob_entry(b".", segment, &flags) {
                    names.push((b".".to_vec(), dot));
                }
                let mut dotdot = d.clone();
                dotdot.push("..");
                if matches_glob_entry(b"..", segment, &flags) {
                    names.push((b"..".to_vec(), dotdot));
                }
            }
            for e in entries.flatten() {
                let name_os = e.file_name();
                let name_bytes = name_os.as_encoded_bytes();
                let name_vec = name_bytes.to_vec();
                if matches_glob_entry_parsed(&name_vec, segment, &segment_tokens, &flags) {
                    names.push((name_vec, e.path()));
                }
            }
            names.sort_by(|a, b| a.0.cmp(&b.0));
            for (_, p) in names {
                next.push(p);
            }
        }
        // If this isn't the last segment, prune non-directories.
        if !is_last {
            next.retain(|p| p.is_dir());
        }
        current_dirs = next;
    }

    sort_glob_paths(&mut current_dirs, flags.globsort);

    let out: Vec<Wd> = current_dirs
        .into_iter()
        .filter_map(|p| {
            let mut bytes = Vec::new();
            let raw = p.as_os_str().as_encoded_bytes();
            if absolute || raw.starts_with(b"./") {
                // strip leading "./" (introduced by the relative anchor) so
                // bash output style matches.
                if raw.starts_with(b"./") && !absolute {
                    bytes.extend_from_slice(&raw[2..]);
                } else {
                    bytes.extend_from_slice(raw);
                }
            } else {
                bytes.extend_from_slice(raw);
            }
            if trailing_slash && bytes.last() != Some(&b'/') {
                bytes.push(b'/');
            }
            if is_ignored_path(&bytes, &flags) {
                None
            } else {
                Some(Wd::from_bytes_with_flags(bytes, wd.flags, wd.span))
            }
        })
        .collect();

    if out.is_empty() {
        return no_glob_match_result(wd, &flags);
    }
    Ok(out)
}

fn parse_globsort(value: &str) -> GlobSort {
    match value {
        "nosort" => GlobSort::NoSort,
        "-name" => GlobSort::NameDesc,
        "+name" | "name" | "" => GlobSort::NameAsc,
        "+atime" | "atime" => GlobSort::AtimeAsc,
        "-atime" => GlobSort::AtimeDesc,
        "+mtime" | "mtime" => GlobSort::MtimeAsc,
        "-mtime" => GlobSort::MtimeDesc,
        "+size" | "size" => GlobSort::SizeAsc,
        "-size" => GlobSort::SizeDesc,
        _ => GlobSort::NameAsc,
    }
}

fn sort_glob_paths(paths: &mut [PathBuf], sort: GlobSort) {
    match sort {
        GlobSort::NoSort => {}
        GlobSort::NameAsc => paths.sort_by_key(|path| path_bytes(path)),
        GlobSort::NameDesc => {
            paths.sort_by_key(|path| std::cmp::Reverse(path_bytes(path)));
        }
        GlobSort::AtimeAsc => paths.sort_by(|a, b| {
            path_atime_key(a)
                .cmp(&path_atime_key(b))
                .then_with(|| path_bytes(a).cmp(&path_bytes(b)))
        }),
        GlobSort::AtimeDesc => paths.sort_by(|a, b| {
            path_atime_key(b)
                .cmp(&path_atime_key(a))
                .then_with(|| path_bytes(b).cmp(&path_bytes(a)))
        }),
        GlobSort::MtimeAsc => paths.sort_by(|a, b| {
            path_mtime_key(a)
                .cmp(&path_mtime_key(b))
                .then_with(|| path_bytes(a).cmp(&path_bytes(b)))
        }),
        GlobSort::MtimeDesc => paths.sort_by(|a, b| {
            path_mtime_key(b)
                .cmp(&path_mtime_key(a))
                .then_with(|| path_bytes(b).cmp(&path_bytes(a)))
        }),
        GlobSort::SizeAsc => paths.sort_by(|a, b| {
            path_size_key(a)
                .cmp(&path_size_key(b))
                .then_with(|| path_bytes(a).cmp(&path_bytes(b)))
        }),
        GlobSort::SizeDesc => paths.sort_by(|a, b| {
            path_size_key(b)
                .cmp(&path_size_key(a))
                .then_with(|| path_bytes(b).cmp(&path_bytes(a)))
        }),
    }
}

fn path_bytes(path: &Path) -> Vec<u8> {
    let raw = path.as_os_str().as_encoded_bytes();
    if raw.starts_with(b"./") {
        raw[2..].to_vec()
    } else {
        raw.to_vec()
    }
}

fn path_atime_key(path: &Path) -> (i64, i64) {
    path.metadata()
        .map(|m| (m.atime(), m.atime_nsec()))
        .unwrap_or_default()
}

fn path_mtime_key(path: &Path) -> (i64, i64) {
    path.metadata()
        .map(|m| (m.mtime(), m.mtime_nsec()))
        .unwrap_or_default()
}

fn path_size_key(path: &Path) -> u64 {
    path.metadata().map(|m| m.len()).unwrap_or_default()
}

fn no_glob_match_result(wd: &Wd, flags: &GlobFlags) -> Result<Vec<Wd>, ExpandError> {
    if flags.failglob {
        let pat = String::from_utf8_lossy(&dequote_bytes(wd.buf.as_bytes())).into_owned();
        return Err(ExpandError::FailGlob(pat));
    }
    if flags.nullglob {
        return Ok(Vec::new());
    }
    Ok(vec![wd.clone()])
}

fn dequote_glob_literal_segment(segment: &[u8]) -> Vec<u8> {
    let dequoted = dequote_bytes(segment);
    let mut out = Vec::with_capacity(dequoted.len());
    let mut i = 0usize;
    while i < dequoted.len() {
        if dequoted[i] == b'\\' && i + 1 < dequoted.len() {
            out.push(dequoted[i + 1]);
            i += 2;
        } else {
            out.push(dequoted[i]);
            i += 1;
        }
    }
    out
}

fn matches_glob_entry(name: &[u8], segment: &[u8], flags: &GlobFlags) -> bool {
    let tokens = parse(segment, flags.opts);
    matches_glob_entry_parsed(name, segment, &tokens, flags)
}

fn matches_glob_entry_parsed(
    name: &[u8],
    segment: &[u8],
    segment_tokens: &[Tok],
    flags: &GlobFlags,
) -> bool {
    if name == b"." || name == b".." {
        if flags.globskipdots || !explicitly_matches_dot_name(segment, name, flags.opts) {
            return false;
        }
    } else if name.first() == Some(&b'.')
        && !flags.dotglob
        && !explicitly_matches_dot_name_parsed(segment_tokens, name, flags.opts)
    {
        return false;
    }
    fnmatch_parsed(segment_tokens, name, flags.opts)
}

fn split_globignore(value: &str) -> Vec<&[u8]> {
    let bytes = value.as_bytes();
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut bracket_depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                i += 2;
                continue;
            }
            b'[' => {
                bracket_depth += 1;
            }
            b']' if bracket_depth > 0 => {
                bracket_depth -= 1;
            }
            b':' if bracket_depth == 0 => {
                pieces.push(&bytes[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    pieces.push(&bytes[start..]);
    pieces
}

fn collect_recursive_dirs(
    start: &Path,
    out: &mut Vec<PathBuf>,
    flags: &GlobFlags,
    include_symlink_dirs: bool,
) {
    out.push(start.to_path_buf());
    let entries = match std::fs::read_dir(start) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    let mut subdirs: Vec<(PathBuf, bool)> = Vec::new();
    for e in entries.flatten() {
        if let Ok(ft) = e.file_type() {
            if ft.is_dir() || (include_symlink_dirs && ft.is_symlink() && e.path().is_dir()) {
                let name = e.file_name();
                let bytes = name.as_encoded_bytes();
                if bytes == b"." || bytes == b".." {
                    continue;
                }
                if !flags.dotglob && bytes.first() == Some(&b'.') {
                    continue;
                }
                subdirs.push((e.path(), ft.is_dir()));
            }
        }
    }
    subdirs.sort_by(|a, b| a.0.cmp(&b.0));
    for (d, descend) in subdirs {
        if descend {
            collect_recursive_dirs(&d, out, flags, include_symlink_dirs);
        } else {
            out.push(d);
        }
    }
}

fn collect_recursive_all(
    start: &Path,
    out: &mut Vec<PathBuf>,
    flags: &GlobFlags,
    include_start: bool,
    slash_start: bool,
) {
    if include_start {
        if slash_start {
            out.push(path_with_trailing_slash(start));
        } else {
            out.push(start.to_path_buf());
        }
    }
    let entries = match std::fs::read_dir(start) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    let mut children: Vec<(PathBuf, bool)> = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name();
        let bytes = name.as_encoded_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        if !flags.dotglob && bytes.first() == Some(&b'.') {
            continue;
        }
        let Ok(file_type) = e.file_type() else {
            continue;
        };
        children.push((e.path(), file_type.is_dir()));
    }
    children.sort_by(|a, b| a.0.cmp(&b.0));
    for (child, descend) in children {
        if descend {
            collect_recursive_all(&child, out, flags, true, false);
        } else {
            out.push(child);
        }
    }
}

fn path_with_trailing_slash(path: &Path) -> PathBuf {
    let mut bytes = path.as_os_str().as_encoded_bytes().to_vec();
    if bytes.last() != Some(&b'/') {
        bytes.push(b'/');
    }
    PathBuf::from(OsString::from_vec(bytes))
}

fn is_ignored_path(path: &[u8], flags: &GlobFlags) -> bool {
    flags
        .globignore
        .iter()
        .any(|p| globignore_match(p, path, flags.opts))
}

fn globignore_match(pattern: &[u8], path: &[u8], opts: GlobOpts) -> bool {
    if !pattern.contains(&b'/') || !path.contains(&b'/') {
        return fnmatch(pattern, path, opts);
    }
    if let Some(collapsed) = collapse_literal_slash_extglob(pattern) {
        return globignore_match(&collapsed, path, opts);
    }
    let pat_parts = pattern.split(|b| *b == b'/');
    let path_parts = path.split(|b| *b == b'/');
    pat_parts
        .zip(path_parts)
        .all(|(pat, text)| fnmatch(pat, text, opts))
        && pattern.split(|b| *b == b'/').count() == path.split(|b| *b == b'/').count()
}

fn collapse_literal_slash_extglob(pattern: &[u8]) -> Option<Vec<u8>> {
    let marker = b"@(/)";
    let pos = pattern
        .windows(marker.len())
        .position(|window| window == marker)?;
    let segment_start = pattern[..pos]
        .iter()
        .rposition(|b| *b == b'/')
        .map(|idx| idx + 1)
        .unwrap_or(0);
    if has_glob_meta(&pattern[segment_start..pos], GlobOpts::default()) {
        return None;
    }
    let mut out = Vec::with_capacity(pattern.len() - marker.len() + 1);
    out.extend_from_slice(&pattern[..pos]);
    out.push(b'/');
    out.extend_from_slice(&pattern[pos + marker.len()..]);
    Some(out)
}

/// Convenience for case/`[[ =~ ]]` matching: anchored fnmatch on `String`s.
pub fn matches(pattern: &str, text: &str) -> bool {
    fnmatch(pattern.as_bytes(), text.as_bytes(), GlobOpts::default())
}
