use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::{workspace_root, HarnessError};

const FILE_METADATA_FIELDS: &[&str] = &[
    "our_shell",
    "compare_shells",
    "suite",
    "tags",
    "oils_failures_allowed",
    "oils_cpp_failures_allowed",
    "legacy_tmp_dir",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OilsCase {
    pub source_file: PathBuf,
    pub case_index: usize,
    pub line_number: usize,
    pub description: String,
    pub code: String,
    pub tags: Vec<String>,
    pub legacy_tmp_dir: bool,
}

impl OilsCase {
    pub fn id(&self) -> String {
        format!(
            "{}::{:03}::{}",
            self.source_file.display(),
            self.case_index,
            self.description
        )
    }
}

pub fn default_oils_spec_dir() -> PathBuf {
    workspace_root().join("vendor/oils/spec")
}

pub fn discover_oils_cases(spec_dir: &Path) -> Result<Vec<OilsCase>, HarnessError> {
    let mut files = fs::read_dir(spec_dir)
        .map_err(HarnessError::Io)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".test.sh"))
        })
        .collect::<Vec<_>>();
    files.sort();

    let mut cases = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path).map_err(HarnessError::Io)?;
        let source_file = path
            .strip_prefix(spec_dir)
            .unwrap_or(&path)
            .to_path_buf();
        cases.extend(parse_oils_spec_source(&source_file, &text)?);
    }
    Ok(cases)
}

pub fn parse_oils_spec_source(
    source_file: &Path,
    text: &str,
) -> Result<Vec<OilsCase>, HarnessError> {
    let (metadata, parsed) = parse_spec_file(source_file, text)?;
    let selected = metadata
        .get("compare_shells")
        .is_some_and(|shells| shells.split_whitespace().any(|shell| shell.starts_with("bash")));
    if !selected {
        return Ok(Vec::new());
    }
    let tags: Vec<String> = metadata
        .get("tags")
        .map(|value| value.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    let legacy_tmp_dir = metadata
        .get("legacy_tmp_dir")
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes"));

    Ok(parsed
        .into_iter()
        .enumerate()
        .map(|(case_index, parsed_case)| OilsCase {
            source_file: source_file.to_path_buf(),
            case_index,
            line_number: parsed_case.line_number,
            description: parsed_case.description,
            code: parsed_case.code,
            tags: tags.clone(),
            legacy_tmp_dir,
        })
        .collect())
}

#[derive(Debug)]
struct ParsedCase {
    line_number: usize,
    description: String,
    code: String,
}

fn parse_spec_file(
    path: &Path,
    text: &str,
) -> Result<(BTreeMap<String, String>, Vec<ParsedCase>), HarnessError> {
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    let first_case = lines
        .iter()
        .position(|line| line.starts_with("####"))
        .unwrap_or(lines.len());
    let mut metadata = BTreeMap::new();
    for (line_index, line) in lines[..first_case].iter().enumerate() {
        if let Some((name, value)) = unqualified_key_value(line) {
            if !FILE_METADATA_FIELDS.contains(&name.as_str()) {
                return Err(invalid_data(format!(
                    "{}:{}: invalid Oils file metadata {name:?}",
                    path.display(),
                    line_index + 1
                )));
            }
            metadata.insert(name, value);
        } else if line.starts_with("##") && !line.starts_with("####") {
            return Err(invalid_data(format!(
                "{}:{}: invalid Oils metadata line",
                path.display(),
                line_index + 1
            )));
        }
    }

    let mut cases = Vec::new();
    let mut cursor = first_case;
    while cursor < lines.len() {
        while cursor < lines.len()
            && (lines[cursor].trim().is_empty() || is_comment(lines[cursor]))
        {
            cursor += 1;
        }
        if cursor == lines.len() {
            break;
        }
        if !lines[cursor].starts_with("####") {
            return Err(invalid_data(format!(
                "{}:{}: expected Oils case header",
                path.display(),
                cursor + 1
            )));
        }
        let line_number = cursor + 1;
        let description = lines[cursor][4..].trim().to_string();
        cursor += 1;
        let case_end = lines[cursor..]
            .iter()
            .position(|line| line.starts_with("####"))
            .map(|offset| cursor + offset)
            .unwrap_or(lines.len());
        let code = parse_case_code(path, line_number, &lines[cursor..case_end])?;
        cases.push(ParsedCase {
            line_number,
            description,
            code,
        });
        cursor = case_end;
    }
    Ok((metadata, cases))
}

fn parse_case_code(path: &Path, line_number: usize, lines: &[&str]) -> Result<String, HarnessError> {
    validate_case_metadata(path, line_number, lines)?;
    let mut cursor = 0;
    while cursor < lines.len() {
        let line = lines[cursor];
        if line.trim().is_empty() || is_comment(line) {
            cursor += 1;
            continue;
        }
        if let Some((name, value)) = any_key_value(line) {
            if name == "code" {
                return Ok(value);
            }
            cursor = skip_metadata_value(lines, cursor, &name);
            continue;
        }
        if line.starts_with("##") {
            return Err(invalid_data(format!(
                "{}:{line_number}: invalid Oils case metadata",
                path.display()
            )));
        }
        break;
    }

    let mut code = String::new();
    while cursor < lines.len() {
        let line = lines[cursor];
        if any_key_value(line).is_some() || line.trim_start().starts_with("## END") {
            break;
        }
        if !is_comment(line) {
            code.push_str(line);
        }
        cursor += 1;
    }
    if code.is_empty() {
        return Err(invalid_data(format!(
            "{}:{line_number}: Oils case has no code",
            path.display()
        )));
    }
    Ok(code)
}

fn skip_metadata_value(lines: &[&str], cursor: usize, name: &str) -> usize {
    let mut next = cursor + 1;
    if matches!(name, "STDOUT" | "STDERR") {
        while next < lines.len() {
            if lines[next].trim_start().starts_with("## END") {
                return next + 1;
            }
            if any_key_value(lines[next]).is_some() {
                return next;
            }
            next += 1;
        }
    }
    next
}

fn unqualified_key_value(line: &str) -> Option<(String, String)> {
    let metadata = metadata_line(line)?;
    (metadata.qualifier.is_none() && metadata.shells.is_none())
        .then_some((metadata.name, metadata.value))
}

fn any_key_value(line: &str) -> Option<(String, String)> {
    let metadata = metadata_line(line)?;
    Some((metadata.name, metadata.value))
}

struct MetadataLine {
    qualifier: Option<String>,
    shells: Option<String>,
    name: String,
    value: String,
}

fn metadata_line(line: &str) -> Option<MetadataLine> {
    let rest = line.strip_prefix("##")?;
    if rest.starts_with('#') || !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    if rest == "END" {
        return None;
    }
    let (prefix, value) = rest.split_once(':')?;
    let fields = prefix.split_whitespace().collect::<Vec<_>>();
    let (qualifier, shells, key) = match fields.as_slice() {
        [key] => (None, None, *key),
        [qualifier, shells, key]
            if qualifier.starts_with("OK")
                || qualifier.starts_with("BUG")
                || *qualifier == "N-I" =>
        {
            (Some((*qualifier).to_string()), Some((*shells).to_string()), *key)
        }
        _ => return None,
    };
    if key.is_empty()
        || !key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return None;
    }
    Some(MetadataLine {
        qualifier,
        shells,
        name: key.to_string(),
        value: value.trim_start().trim_end().to_string(),
    })
}

fn validate_case_metadata(
    path: &Path,
    line_number: usize,
    lines: &[&str],
) -> Result<(), HarnessError> {
    let mut seen = BTreeSet::new();
    let mut qualifiers = BTreeMap::<String, String>::new();
    let mut cursor = 0;
    while cursor < lines.len() {
        let Some(metadata) = metadata_line(lines[cursor]) else {
            cursor += 1;
            continue;
        };
        let logical_name = metadata
            .name
            .strip_suffix("-json")
            .unwrap_or(&metadata.name)
            .to_string();
        let Some(shells) = metadata.shells.as_deref() else {
            cursor = skip_metadata_value(lines, cursor, &metadata.name);
            continue;
        };
        let scopes = shells.split('/').map(str::to_owned).collect::<Vec<_>>();
        for scope in scopes {
            if !seen.insert((scope.clone(), logical_name.clone())) {
                return Err(invalid_data(format!(
                    "{}:{line_number}: duplicate {logical_name} assertion for {scope}",
                    path.display()
                )));
            }
            if let Some(qualifier) = &metadata.qualifier {
                if let Some(previous) = qualifiers.insert(scope.clone(), qualifier.clone()) {
                    if previous != *qualifier {
                        return Err(invalid_data(format!(
                            "{}:{line_number}: inconsistent qualifier for {scope}",
                            path.display()
                        )));
                    }
                }
            }
        }
        cursor = skip_metadata_value(lines, cursor, &metadata.name);
    }
    Ok(())
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#') && !line.starts_with("##")
}

fn invalid_data(message: String) -> HarnessError {
    HarnessError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, message))
}
