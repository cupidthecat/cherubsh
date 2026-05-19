//! `fc` builtin (history edit + re-execute).

use crate::common::report_diagnostic;
use crate::{Builtin, BuiltinCtx};
use cherubsh_common::Environment;

const FC_USAGE: &str = "fc [-e ename] [-lnr] [first] [last] or fc -s [pat=rep] [command]";

pub struct Fc;
pub static FC: Fc = Fc;

impl Builtin for Fc {
    fn name(&self) -> &'static str {
        "fc"
    }
    fn synopsis(&self) -> &'static str {
        FC_USAGE
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let ParsedFcOptions {
            list,
            no_numbers,
            reverse,
            substitute,
            editor,
            rest,
        } = match parse_fc_options(ctx.args, ctx.env_ref()) {
            Ok(opts) => opts,
            Err(status) => return status,
        };
        let table = match ctx.env_ref().history() {
            Some(t) => t,
            None => {
                eprintln!("cherubsh: fc: history not available");
                return 1;
            }
        };
        let current_added = ctx.env_ref().history_last_line_added();
        if list {
            return list_range(
                ctx.env_ref(),
                table,
                &rest,
                no_numbers,
                reverse,
                current_added,
                ctx.env_ref().option("posix"),
            );
        }
        if substitute {
            // -s [pat=rep] [command]
            let (substitutions, cmd_token) = split_substitutions(&rest);
            let last = match find_last_match(table, cmd_token, current_added) {
                Some(s) => s,
                None => {
                    report_diagnostic(ctx.env_ref(), "fc", "no command found");
                    return 1;
                }
            };
            let new_line = apply_substitutions(last, &substitutions);
            eprintln!("{}", new_line);
            let control = ctx.env_ref().histcontrol();
            if let Some(table) = ctx.env().history_mut() {
                table.replace_last(&new_line, control);
            }
            // Execute via the shell ops surface.
            return ctx.shell.run_source(&new_line);
        }
        let target = match edit_target_entry(table, current_added, rest.first().map(String::as_str))
        {
            Ok(Some(e)) => e.line.clone(),
            Ok(None) => return 1,
            Err(FcLookupError::Range) => {
                report_diagnostic(ctx.env_ref(), "fc", "history specification out of range");
                return 1;
            }
            Err(FcLookupError::NotFound) => {
                report_diagnostic(ctx.env_ref(), "fc", "no command found");
                return 1;
            }
        };
        // Default: invoke external editor on the last command, then run.
        let editor = editor
            .or_else(|| std::env::var("FCEDIT").ok())
            .or_else(|| std::env::var("EDITOR").ok())
            .unwrap_or_else(|| "vi".to_string());
        let tmp = std::env::temp_dir().join("cherubsh-fc.tmp");
        let mut edit_text = target;
        if !edit_text.ends_with('\n') {
            edit_text.push('\n');
        }
        if std::fs::write(&tmp, &edit_text).is_err() {
            return 1;
        }
        let status = std::process::Command::new(editor)
            .arg(&tmp)
            .status()
            .map(|s| s.code().unwrap_or(1))
            .unwrap_or(1);
        if status != 0 {
            return status;
        }
        let body = std::fs::read_to_string(&tmp).unwrap_or_default();
        let trimmed = body.trim_end_matches('\n').to_string();
        println!("{}", trimmed);
        ctx.shell.run_source(&trimmed)
    }
}

fn list_range(
    env: &dyn Environment,
    table: &cherubsh_common::history::HistoryTable,
    rest: &[String],
    no_numbers: bool,
    reverse: bool,
    current_added: bool,
    posix: bool,
) -> i32 {
    let total = table.len();
    let effective = effective_total(table, current_added);
    if total == 0 || effective == 0 {
        return 0;
    }
    let first_event = table.base() + 1;
    let last_hist = table.base() + effective;
    let real_last = table.base() + total;
    let (mut start, mut end) = if let Some(a) = rest.first() {
        let start = match resolve_list_index(table, current_added, a, true) {
            Ok(idx) => idx,
            Err(FcLookupError::NotFound) => {
                report_diagnostic(env, "fc", "no command found");
                return 1;
            }
            Err(FcLookupError::Range) => {
                report_diagnostic(env, "fc", "history specification out of range");
                return 1;
            }
        };
        let end = if let Some(b) = rest.get(1) {
            match resolve_list_index(table, current_added, b, false) {
                Ok(idx) => idx,
                Err(FcLookupError::NotFound) => {
                    report_diagnostic(env, "fc", "no command found");
                    return 1;
                }
                Err(FcLookupError::Range) => {
                    report_diagnostic(env, "fc", "history specification out of range");
                    return 1;
                }
            }
        } else if start == real_last {
            real_last
        } else {
            last_hist
        };
        (start, end)
    } else {
        (last_hist.saturating_sub(15).max(first_event), last_hist)
    };
    if end < start {
        std::mem::swap(&mut start, &mut end);
    }
    let mut indices: Vec<usize> = (start..=end.min(real_last)).collect();
    if reverse {
        indices.reverse();
    }
    for event_no in indices {
        if current_added && event_no == real_last && event_no != start && event_no != end {
            continue;
        }
        if !current_added && event_no > last_hist {
            continue;
        }
        if event_no < first_event {
            continue;
        }
        if let Some(entry) = table.get(event_no) {
            let marker = if posix { "\t" } else { "\t " };
            if no_numbers {
                println!("{}{}", marker, entry.line);
            } else {
                println!("{}{}{}", event_no, marker, entry.line);
            }
        }
    }
    0
}

fn resolve_list_index(
    table: &cherubsh_common::history::HistoryTable,
    current_added: bool,
    token: &str,
    first_arg: bool,
) -> Result<usize, FcLookupError> {
    let total = table.len();
    let effective = effective_total(table, current_added);
    if total == 0 || effective == 0 {
        return Err(FcLookupError::Range);
    }
    let first_event = table.base() + 1;
    let last_hist = table.base() + effective;
    let real_last = table.base() + total;
    if is_negative_zero(token) {
        return Ok(real_last);
    }
    if let Some((negative, value)) = parse_numeric_spec(token) {
        if negative {
            let idx = last_hist.saturating_add(1).saturating_sub(value);
            return Ok(idx.max(first_event));
        }
        if value == 0 {
            return Ok(last_hist);
        }
        if value < first_event || value > last_hist {
            return Ok(if first_arg { first_event } else { last_hist });
        }
        return Ok(value);
    }
    for (idx, entry) in table.iter().take(effective).enumerate().rev() {
        if entry.line.starts_with(token) {
            return Ok(table.base() + idx + 1);
        }
    }
    Err(FcLookupError::NotFound)
}

fn find_last_match(
    table: &cherubsh_common::history::HistoryTable,
    token: Option<&str>,
    current_added: bool,
) -> Option<String> {
    let total = effective_total(table, current_added);
    if total == 0 {
        return None;
    }
    let newest = table.base() + total;
    match token {
        None => table.get(newest).map(|e| e.line.clone()),
        Some(t) => {
            if let Some(idx) = parse_execute_index(t, table.base(), total) {
                return table.get(idx).map(|e| e.line.clone());
            }
            for entry in table.iter().take(total).rev() {
                if entry.line.starts_with(t) {
                    return Some(entry.line.clone());
                }
            }
            None
        }
    }
}

fn effective_total(table: &cherubsh_common::history::HistoryTable, current_added: bool) -> usize {
    table.len().saturating_sub(usize::from(current_added))
}

fn last_edit_entry(
    table: &cherubsh_common::history::HistoryTable,
    current_added: bool,
) -> Option<&cherubsh_common::history::HistoryEntry> {
    let total = effective_total(table, current_added);
    if total == 0 {
        return None;
    }
    table.get(table.base() + total)
}

enum FcLookupError {
    Range,
    NotFound,
}

fn edit_target_entry<'a>(
    table: &'a cherubsh_common::history::HistoryTable,
    current_added: bool,
    token: Option<&str>,
) -> Result<Option<&'a cherubsh_common::history::HistoryEntry>, FcLookupError> {
    let Some(token) = token else {
        return Ok(last_edit_entry(table, current_added));
    };
    if is_negative_zero(token) {
        return Err(FcLookupError::Range);
    }
    let total = effective_total(table, current_added);
    if total == 0 {
        return Ok(None);
    }
    if let Some(idx) = parse_execute_index(token, table.base(), total) {
        return Ok(table.get(idx));
    }
    for entry in table.iter().take(total).rev() {
        if entry.line.starts_with(token) {
            return Ok(Some(entry));
        }
    }
    Err(FcLookupError::NotFound)
}

struct ParsedFcOptions {
    list: bool,
    no_numbers: bool,
    reverse: bool,
    substitute: bool,
    editor: Option<String>,
    rest: Vec<String>,
}

fn parse_fc_options(args: &[String], env: &dyn Environment) -> Result<ParsedFcOptions, i32> {
    let mut list = false;
    let mut no_numbers = false;
    let mut reverse = false;
    let mut substitute = false;
    let mut editor: Option<String> = None;
    let mut pos = 0;

    while pos < args.len() {
        let arg = &args[pos];
        if is_fc_number(arg) {
            break;
        }
        if arg == "--" {
            pos += 1;
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            break;
        }

        let chars: Vec<char> = arg.chars().collect();
        let mut inner = 1;
        while inner < chars.len() {
            let ch = chars[inner];
            inner += 1;
            match ch {
                'l' => list = true,
                'n' => no_numbers = true,
                'r' => reverse = true,
                's' => substitute = true,
                'e' => {
                    if inner < chars.len() {
                        editor = Some(chars[inner..].iter().collect());
                        pos += 1;
                    } else {
                        pos += 1;
                        let Some(value) = args.get(pos) else {
                            report_diagnostic(env, "fc", "-e: option requires an argument");
                            eprintln!("fc: usage: {FC_USAGE}");
                            return Err(2);
                        };
                        editor = Some(value.clone());
                        pos += 1;
                    }
                    if editor.as_deref() == Some("-") {
                        substitute = true;
                    }
                    return Ok(ParsedFcOptions {
                        list,
                        no_numbers,
                        reverse,
                        substitute,
                        editor,
                        rest: args[pos..].to_vec(),
                    });
                }
                _ => {
                    report_diagnostic(env, "fc", &format!("-{ch}: invalid option"));
                    eprintln!("fc: usage: {FC_USAGE}");
                    return Err(2);
                }
            }
        }
        pos += 1;
    }

    Ok(ParsedFcOptions {
        list,
        no_numbers,
        reverse,
        substitute,
        editor,
        rest: args[pos..].to_vec(),
    })
}

fn is_fc_number(arg: &str) -> bool {
    let Some(first) = arg.as_bytes().first() else {
        return false;
    };
    let digits = if *first == b'-' { &arg[1..] } else { arg };
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

fn parse_numeric_spec(token: &str) -> Option<(bool, usize)> {
    let (negative, digits) = token
        .strip_prefix('-')
        .map(|rest| (true, rest))
        .unwrap_or((false, token));
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((negative, digits.parse::<usize>().ok()?))
}

fn is_negative_zero(token: &str) -> bool {
    token
        .strip_prefix('-')
        .map(|digits| !digits.is_empty() && digits.bytes().all(|b| b == b'0'))
        .unwrap_or(false)
}

fn split_substitutions(rest: &[String]) -> (Vec<(String, String)>, Option<&str>) {
    let mut substitutions = Vec::new();
    for (idx, arg) in rest.iter().enumerate() {
        let Some((pat, rep)) = arg.split_once('=') else {
            return (substitutions, Some(arg.as_str()));
        };
        substitutions.push((pat.to_string(), rep.to_string()));
        if idx == rest.len() - 1 {
            return (substitutions, None);
        }
    }
    (substitutions, None)
}

fn apply_substitutions(mut line: String, substitutions: &[(String, String)]) -> String {
    for (pat, rep) in substitutions {
        line = line.replace(pat, rep);
    }
    line
}

fn parse_execute_index(token: &str, base: usize, total: usize) -> Option<usize> {
    let newest = base + total;
    let first = base + 1;
    let (negative, digits) = token
        .strip_prefix('-')
        .map(|rest| (true, rest))
        .unwrap_or((false, token));
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if negative && digits.bytes().all(|b| b == b'0') {
        return None;
    }
    let value = digits.parse::<usize>().ok()?;
    if negative {
        return Some(newest.saturating_add(1).saturating_sub(value).max(first));
    }
    if value == 0 {
        return Some(newest);
    }
    Some(value.clamp(first, newest))
}
