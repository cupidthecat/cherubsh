//! Command-history data model.
//!
//! Ring buffer of `HistoryEntry` capped at `$HISTSIZE`. The shell crate is
//! the only writer; builtins (`history`, `fc`) read through the
//! `Environment` surface. File I/O lives here so the test harness can call
//! it without bringing in `crates/shell`.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use bitflags::bitflags;

bitflags! {
    /// `$HISTCONTROL` flags. Mirrors bash bashhist.c `hc_*` bits.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct HistControl: u32 {
        const IGNORESPACE = 1 << 0;
        const IGNOREDUPS  = 1 << 1;
        const ERASEDUPS   = 1 << 2;
    }
}

impl HistControl {
    pub fn parse(value: &str) -> Self {
        let mut out = Self::empty();
        for tok in value.split(':') {
            match tok.trim() {
                "ignorespace" => out |= Self::IGNORESPACE,
                "ignoredups" => out |= Self::IGNOREDUPS,
                "ignoreboth" => out |= Self::IGNORESPACE | Self::IGNOREDUPS,
                "erasedups" => out |= Self::ERASEDUPS,
                _ => {}
            }
        }
        out
    }
}

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub line: String,
    pub timestamp: Option<u64>,
    pub timestamp_from_file: bool,
}

#[derive(Default, Debug)]
pub struct HistoryTable {
    entries: Vec<HistoryEntry>,
    max: usize,
    /// Number of entries discarded from the front of the ring. Bash keeps
    /// event numbers monotonic even after HISTSIZE trimming.
    base: usize,
    /// Marker - index of first entry not yet written to disk by
    /// `history -a`. Tracks bash's `history_lines_this_session` counter.
    new_since_save: usize,
}

impl HistoryTable {
    pub fn new(max: usize) -> Self {
        Self {
            entries: Vec::new(),
            max: max.max(1),
            base: 0,
            new_since_save: 0,
        }
    }

    pub fn set_max(&mut self, max: usize) {
        self.max = max.max(1);
        self.trim();
    }

    pub fn max(&self) -> usize {
        self.max
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn base(&self) -> usize {
        self.base
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, HistoryEntry> {
        self.entries.iter()
    }

    pub fn get(&self, idx_one_based: usize) -> Option<&HistoryEntry> {
        if idx_one_based == 0 || idx_one_based <= self.base {
            return None;
        }
        self.entries.get(idx_one_based - self.base - 1)
    }

    pub fn last(&self) -> Option<&HistoryEntry> {
        self.entries.last()
    }

    pub fn nth_last(&self, n: usize) -> Option<&HistoryEntry> {
        if n == 0 || n > self.entries.len() {
            return None;
        }
        self.entries.get(self.entries.len() - n)
    }

    /// Add an entry honouring HISTCONTROL flags. Returns true if accepted.
    pub fn add(&mut self, line: &str, control: HistControl) -> bool {
        if line.is_empty() {
            return false;
        }
        if control.contains(HistControl::IGNORESPACE) && line.starts_with(' ') {
            return false;
        }
        if control.contains(HistControl::IGNOREDUPS) {
            if let Some(last) = self.entries.last() {
                if last.line == line {
                    return false;
                }
            }
        }
        if control.contains(HistControl::ERASEDUPS) {
            self.entries.retain(|e| e.line != line);
        }
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs());
        self.entries.push(HistoryEntry {
            line: line.to_string(),
            timestamp: ts,
            timestamp_from_file: false,
        });
        self.trim();
        true
    }

    /// Bypass HISTCONTROL - used by `history -s` and history file load.
    pub fn add_forced(&mut self, line: &str, timestamp: Option<u64>) {
        self.entries.push(HistoryEntry {
            line: line.to_string(),
            timestamp,
            timestamp_from_file: false,
        });
        self.trim();
    }

    pub fn remove_at(&mut self, idx_one_based: usize) -> bool {
        if idx_one_based == 0 || idx_one_based <= self.base {
            return false;
        }
        let idx = idx_one_based - self.base - 1;
        if idx >= self.entries.len() {
            return false;
        }
        self.entries.remove(idx);
        true
    }

    pub fn remove_range(&mut self, start: usize, end: usize) {
        let first = self.base + 1;
        let last = self.base + self.entries.len();
        if start > last || end < first {
            return;
        }
        let s = start.max(first) - first;
        let e = end.min(last) - self.base;
        if s < e {
            self.entries.drain(s..e);
        }
    }

    pub fn replace_last(&mut self, line: &str, control: HistControl) -> bool {
        if self.entries.pop().is_some() && self.new_since_save > self.entries.len() {
            self.new_since_save = self.entries.len();
        }
        self.add(line, control)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.base = 0;
        self.new_since_save = 0;
    }

    pub fn mark_saved(&mut self) {
        self.new_since_save = self.entries.len();
    }

    pub fn new_since_save(&self) -> usize {
        self.entries.len().saturating_sub(self.new_since_save)
    }

    fn trim(&mut self) {
        while self.entries.len() > self.max {
            self.entries.remove(0);
            self.base += 1;
            if self.new_since_save > 0 {
                self.new_since_save -= 1;
            }
        }
    }

    /// Load history file. Format: lines of text, optionally preceded by
    /// `#<timestamp>` comment lines (when HISTTIMEFORMAT is set in the
    /// originating shell). Append; does not clear existing entries.
    pub fn load_from(&mut self, path: &Path) -> io::Result<()> {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let reader = BufReader::new(file);
        let mut pending_ts: Option<u64> = None;
        let mut entry = String::new();
        let mut entry_ts: Option<u64> = None;
        for line in reader.lines() {
            let line = line?;
            if entry.is_empty() && line.is_empty() {
                if pending_ts.is_none() {
                    if let Some(last) = self.entries.last_mut() {
                        last.line.push('\n');
                    }
                }
                continue;
            }
            if entry.is_empty() {
                if let Some(rest) = line.strip_prefix('#') {
                    if let Ok(ts) = rest.parse::<u64>() {
                        pending_ts = Some(ts);
                        continue;
                    }
                }
                entry_ts = pending_ts.take();
                entry.push_str(&line);
            } else {
                entry.push('\n');
                entry.push_str(&line);
            }

            if history_entry_complete(&entry) {
                self.entries.push(HistoryEntry {
                    line: std::mem::take(&mut entry),
                    timestamp: entry_ts.take(),
                    timestamp_from_file: true,
                });
            }
        }
        if !entry.is_empty() {
            self.entries.push(HistoryEntry {
                line: entry,
                timestamp: entry_ts,
                timestamp_from_file: true,
            });
        }
        self.trim();
        self.new_since_save = self.entries.len();
        Ok(())
    }

    /// Load history file without reconstructing multiline entries. Format:
    /// lines of text, optionally preceded by `#<timestamp>` comment lines.
    pub fn load_physical_lines_from(&mut self, path: &Path) -> io::Result<()> {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let reader = BufReader::new(file);
        let mut pending_ts: Option<u64> = None;
        for line in reader.lines() {
            let line = line?;
            if let Some(rest) = line.strip_prefix('#') {
                if let Ok(ts) = rest.parse::<u64>() {
                    pending_ts = Some(ts);
                    continue;
                }
            }
            self.entries.push(HistoryEntry {
                line,
                timestamp: pending_ts.take(),
                timestamp_from_file: true,
            });
        }
        self.trim();
        self.new_since_save = self.entries.len();
        Ok(())
    }

    /// Write to disk. `max_size` caps total lines (HISTFILESIZE).
    pub fn write_to(&self, path: &Path, max_size: usize, with_timestamps: bool) -> io::Result<()> {
        let mut f = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(path)?;
        let start = self.entries.len().saturating_sub(max_size);
        for entry in &self.entries[start..] {
            if with_timestamps || entry.timestamp_from_file {
                if let Some(ts) = entry.timestamp {
                    writeln!(f, "#{}", ts)?;
                }
            }
            writeln!(f, "{}", entry.line)?;
        }
        Ok(())
    }

    /// Append only entries new since last `mark_saved` (used by `history -a`).
    pub fn append_to(&mut self, path: &Path, with_timestamps: bool) -> io::Result<()> {
        let new = self.new_since_save();
        if new == 0 {
            return Ok(());
        }
        let mut f = OpenOptions::new().append(true).create(true).open(path)?;
        let start = self.entries.len() - new;
        for entry in &self.entries[start..] {
            if with_timestamps || entry.timestamp_from_file {
                if let Some(ts) = entry.timestamp {
                    writeln!(f, "#{}", ts)?;
                }
            }
            writeln!(f, "{}", entry.line)?;
        }
        self.mark_saved();
        Ok(())
    }
}

fn history_entry_complete(input: &str) -> bool {
    quotes_balanced(input) && heredocs_closed(input)
}

fn quotes_balanced(input: &str) -> bool {
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    for byte in input.bytes() {
        if escaped {
            escaped = false;
            continue;
        }
        match quote {
            Some(b'\'') => {
                if byte == b'\'' {
                    quote = None;
                }
            }
            Some(b'"') => {
                if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    quote = None;
                }
            }
            _ => {
                if byte == b'\\' {
                    escaped = true;
                } else if byte == b'\'' || byte == b'"' {
                    quote = Some(byte);
                }
            }
        }
    }
    quote.is_none() && !escaped
}

fn heredocs_closed(input: &str) -> bool {
    let mut pending: Vec<String> = Vec::new();
    for line in input.lines() {
        if let Some(delim) = pending.first() {
            if line == delim {
                pending.remove(0);
                continue;
            }
            continue;
        }
        pending.extend(extract_heredoc_delimiters(line));
    }
    pending.is_empty()
}

fn extract_heredoc_delimiters(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] != b'<' || bytes[i + 1] != b'<' {
            i += 1;
            continue;
        }
        if i + 2 < bytes.len() && bytes[i + 2] == b'<' {
            i += 3;
            continue;
        }
        i += 2;
        if i < bytes.len() && bytes[i] == b'-' {
            i += 1;
        }
        while i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !matches!(bytes[i], b' ' | b'\t' | b';' | b'|' | b'&') {
            i += 1;
        }
        if i > start {
            let raw = &line[start..i];
            let delimiter = raw
                .chars()
                .filter(|ch| !matches!(ch, '\'' | '"' | '\\'))
                .collect::<String>();
            if !delimiter.is_empty() {
                out.push(delimiter);
            }
        }
    }
    out
}
