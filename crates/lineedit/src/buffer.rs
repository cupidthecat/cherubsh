//! Line buffer with cursor + undo.

use crate::Completion;

#[derive(Clone, Default)]
pub struct EditBuffer {
    chars: Vec<char>,
    point: usize,
    undo_stack: Vec<(Vec<char>, usize)>,
    coalescing_insert: bool,
}

impl EditBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contents(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn as_str(&self) -> String {
        self.contents()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn point(&self) -> usize {
        self.byte_point()
    }

    pub fn char_point(&self) -> usize {
        self.point
    }

    fn byte_point(&self) -> usize {
        self.chars[..self.point].iter().map(|c| c.len_utf8()).sum()
    }

    fn snapshot(&mut self) {
        self.coalescing_insert = false;
        self.undo_stack.push((self.chars.clone(), self.point));
        if self.undo_stack.len() > 128 {
            self.undo_stack.remove(0);
        }
    }

    fn snapshot_insert(&mut self) {
        if !self.coalescing_insert {
            self.snapshot();
            self.coalescing_insert = true;
        }
    }

    pub fn break_undo_group(&mut self) {
        self.coalescing_insert = false;
    }

    pub fn undo(&mut self) {
        self.coalescing_insert = false;
        if let Some((chars, point)) = self.undo_stack.pop() {
            self.chars = chars;
            self.point = point;
        }
    }

    pub fn clear(&mut self) {
        self.snapshot();
        self.chars.clear();
        self.point = 0;
    }

    pub fn insert(&mut self, c: char) {
        self.snapshot_insert();
        self.chars.insert(self.point, c);
        self.point += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        self.snapshot_insert();
        for c in s.chars() {
            self.chars.insert(self.point, c);
            self.point += 1;
        }
    }

    pub fn delete_forward(&mut self) {
        if self.point >= self.chars.len() {
            return;
        }
        self.snapshot();
        self.chars.remove(self.point);
    }

    pub fn backward_delete(&mut self) {
        if self.point == 0 {
            return;
        }
        self.snapshot();
        self.point -= 1;
        self.chars.remove(self.point);
    }

    pub fn move_left(&mut self) {
        if self.point > 0 {
            self.point -= 1;
        }
    }

    pub fn move_right(&mut self) {
        if self.point < self.chars.len() {
            self.point += 1;
        }
    }

    pub fn move_home(&mut self) {
        self.point = 0;
    }

    pub fn move_end(&mut self) {
        self.point = self.chars.len();
    }

    pub fn set_byte_point(&mut self, byte: usize) {
        self.point = self.char_index_for_byte(byte);
    }

    pub fn set_char_point(&mut self, point: usize) {
        self.point = point.min(self.chars.len());
    }

    pub fn char_at(&self, point: usize) -> Option<char> {
        self.chars.get(point).copied()
    }

    pub fn slice(&self, start: usize, end: usize) -> String {
        let start = start.min(self.chars.len());
        let end = end.min(self.chars.len()).max(start);
        self.chars[start..end].iter().collect()
    }

    pub fn move_word_left(&mut self) {
        while self.point > 0 && !is_word_char(self.chars[self.point - 1]) {
            self.point -= 1;
        }
        while self.point > 0 && is_word_char(self.chars[self.point - 1]) {
            self.point -= 1;
        }
    }

    pub fn move_word_right(&mut self) {
        while self.point < self.chars.len() && !is_word_char(self.chars[self.point]) {
            self.point += 1;
        }
        while self.point < self.chars.len() && is_word_char(self.chars[self.point]) {
            self.point += 1;
        }
    }

    pub fn word_left_point(&self) -> usize {
        let mut point = self.point;
        while point > 0 && !is_word_char(self.chars[point - 1]) {
            point -= 1;
        }
        while point > 0 && is_word_char(self.chars[point - 1]) {
            point -= 1;
        }
        point
    }

    pub fn word_right_point(&self) -> usize {
        let mut point = self.point;
        while point < self.chars.len() && !is_word_char(self.chars[point]) {
            point += 1;
        }
        while point < self.chars.len() && is_word_char(self.chars[point]) {
            point += 1;
        }
        point
    }

    pub fn move_visual_line(&mut self, columns: usize, direction: i32) {
        let columns = columns.max(1);
        let row = self.point / columns;
        let column = self.point % columns;
        let target_row = if direction < 0 {
            row.saturating_sub(1)
        } else {
            row.saturating_add(1)
        };
        self.point = (target_row * columns + column).min(self.chars.len());
    }

    pub fn find_forward(&self, needle: char, include: bool) -> Option<usize> {
        self.chars
            .iter()
            .enumerate()
            .skip(self.point.saturating_add(1))
            .find_map(|(index, ch)| {
                (*ch == needle).then_some(if include {
                    index
                } else {
                    index.saturating_sub(1)
                })
            })
    }

    pub fn find_backward(&self, needle: char, include: bool) -> Option<usize> {
        self.chars[..self.point]
            .iter()
            .rposition(|ch| *ch == needle)
            .map(|index| {
                if include {
                    index
                } else {
                    index.saturating_add(1)
                }
            })
    }

    pub fn kill_to_end(&mut self) -> String {
        self.snapshot();
        let tail: String = self.chars[self.point..].iter().collect();
        self.chars.truncate(self.point);
        tail
    }

    pub fn kill_to_beginning(&mut self) -> String {
        self.snapshot();
        let head: String = self.chars[..self.point].iter().collect();
        self.chars.drain(..self.point);
        self.point = 0;
        head
    }

    pub fn backward_kill_word(&mut self) -> String {
        let start = self.point;
        self.move_word_left();
        let end = start;
        self.snapshot();
        let killed: String = self.chars[self.point..end].iter().collect();
        self.chars.drain(self.point..end);
        killed
    }

    pub fn kill_range(&mut self, left: usize, right: usize) -> String {
        let left = left.min(self.chars.len());
        let right = right.min(self.chars.len());
        let (start, end) = if left <= right {
            (left, right)
        } else {
            (right, left)
        };
        if start == end {
            return String::new();
        }
        self.snapshot();
        let killed = self.chars[start..end].iter().collect();
        self.chars.drain(start..end);
        self.point = start;
        killed
    }

    pub fn replace_char_range(&mut self, start: usize, end: usize, value: &str) {
        let start = start.min(self.chars.len());
        let end = end.min(self.chars.len()).max(start);
        self.snapshot();
        self.chars.splice(start..end, value.chars());
        self.point = start + value.chars().count();
    }

    pub fn forward_kill_word(&mut self) -> String {
        let start = self.point;
        self.move_word_right();
        let end = self.point;
        self.point = start;
        self.snapshot();
        let killed: String = self.chars[start..end].iter().collect();
        self.chars.drain(start..end);
        killed
    }

    pub fn transpose_chars(&mut self) {
        if self.chars.len() < 2 {
            return;
        }
        let idx = if self.point >= self.chars.len() {
            self.chars.len() - 1
        } else {
            self.point.max(1)
        };
        if idx == 0 {
            return;
        }
        self.snapshot();
        self.chars.swap(idx - 1, idx);
        if self.point < self.chars.len() {
            self.point += 1;
        }
    }

    pub fn transpose_words(&mut self) {
        let ranges = word_ranges(&self.chars);
        if ranges.len() < 2 {
            return;
        }
        let right_index = ranges
            .iter()
            .position(|(start, end)| *start <= self.point && self.point <= *end)
            .or_else(|| ranges.iter().position(|(start, _)| *start > self.point))
            .unwrap_or(ranges.len() - 1);
        if right_index == 0 {
            return;
        }
        let (left_start, left_end) = ranges[right_index - 1];
        let (right_start, right_end) = ranges[right_index];
        let left: String = self.chars[left_start..left_end].iter().collect();
        let middle: String = self.chars[left_end..right_start].iter().collect();
        let right: String = self.chars[right_start..right_end].iter().collect();
        self.snapshot();
        let replacement = format!("{right}{middle}{left}");
        self.chars
            .splice(left_start..right_end, replacement.chars());
        self.point = right_end;
    }

    fn case_map_word<F: Fn(char) -> char>(&mut self, f: F) {
        let start = self.point;
        self.move_word_right();
        let end = self.point;
        self.snapshot();
        for i in start..end {
            self.chars[i] = f(self.chars[i]);
        }
    }

    pub fn upcase_word(&mut self) {
        self.case_map_word(|c| c.to_ascii_uppercase());
    }

    pub fn downcase_word(&mut self) {
        self.case_map_word(|c| c.to_ascii_lowercase());
    }

    pub fn capitalize_word(&mut self) {
        let start = self.point;
        self.move_word_right();
        let end = self.point;
        self.snapshot();
        let mut seen_alpha = false;
        for i in start..end {
            if self.chars[i].is_alphabetic() {
                if !seen_alpha {
                    self.chars[i] = self.chars[i].to_ascii_uppercase();
                    seen_alpha = true;
                } else {
                    self.chars[i] = self.chars[i].to_ascii_lowercase();
                }
            } else {
                seen_alpha = false;
            }
        }
    }

    pub fn replace_all(&mut self, s: &str) {
        self.snapshot();
        self.chars = s.chars().collect();
        self.point = self.chars.len();
    }

    pub fn replace_all_at_byte(&mut self, value: &str, point: usize) {
        self.replace_all(value);
        self.set_byte_point(point);
    }

    pub fn apply_completion(&mut self, completion: &Completion) {
        if completion.matches.is_empty() {
            return;
        }
        let word_start = self.char_index_for_byte(completion.replace_start);
        let cur_word: String = self.chars[word_start..self.point].iter().collect();
        let common = common_prefix(&completion.matches);
        let chosen = if completion.matches.len() == 1 {
            completion.matches[0].clone()
        } else if common.len() > cur_word.len() {
            common
        } else {
            use std::io::Write;
            let _ = writeln!(std::io::stderr());
            for m in &completion.matches {
                let _ = writeln!(std::io::stderr(), "{}", m);
            }
            return;
        };
        self.snapshot();
        let new_chars: Vec<char> = chosen.chars().collect();
        let inserted = new_chars.len();
        let head: String = self.chars[..word_start].iter().collect();
        let tail: String = self.chars[self.point..].iter().collect();
        let rebuilt = format!("{head}{chosen}{tail}");
        self.chars = rebuilt.chars().collect();
        self.point = word_start + inserted;
        if completion.matches.len() == 1
            && !completion.suppress_append
            && !(completion.filenames && chosen.ends_with('/'))
        {
            if let Some(ch) = completion.append_character {
                self.chars.insert(self.point, ch);
                self.point += 1;
            }
        }
    }

    pub fn replace_completion(&mut self, completion: &Completion, chosen: &str) {
        let start = self.char_index_for_byte(completion.replace_start);
        self.replace_char_range(start, self.point, chosen);
        if !(completion.suppress_append || completion.filenames && chosen.ends_with('/')) {
            if let Some(ch) = completion.append_character {
                self.insert(ch);
            }
        }
    }

    fn char_index_for_byte(&self, byte: usize) -> usize {
        let mut offset = 0usize;
        for (index, ch) in self.chars.iter().enumerate() {
            if offset >= byte {
                return index;
            }
            offset += ch.len_utf8();
        }
        self.chars.len()
    }
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn word_ranges(chars: &[char]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        while index < chars.len() && !is_word_char(chars[index]) {
            index += 1;
        }
        let start = index;
        while index < chars.len() && is_word_char(chars[index]) {
            index += 1;
        }
        if start < index {
            ranges.push((start, index));
        }
    }
    ranges
}

fn common_prefix(matches: &[String]) -> String {
    if matches.is_empty() {
        return String::new();
    }
    let first = &matches[0];
    let mut end = first.len();
    for s in &matches[1..] {
        let mut new_end = 0;
        for (a, b) in first.chars().zip(s.chars()) {
            if a == b {
                new_end += a.len_utf8();
            } else {
                break;
            }
        }
        end = end.min(new_end);
        if end == 0 {
            break;
        }
    }
    first[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::EditBuffer;
    use crate::Completion;

    #[test]
    fn completion_preserves_tail_after_cursor() {
        let mut buf = EditBuffer::new();
        buf.insert_str("echo appl suffix");
        for _ in 0.." suffix".len() {
            buf.move_left();
        }
        buf.apply_completion(&Completion {
            matches: vec!["apple".to_string()],
            replace_start: 5,
            append_character: Some(' '),
            ..Completion::default()
        });
        assert_eq!(buf.contents(), "echo apple  suffix");
    }

    #[test]
    fn completion_uses_byte_offsets_for_utf8_lines() {
        let mut buf = EditBuffer::new();
        buf.insert_str("écho appl");
        buf.apply_completion(&Completion {
            matches: vec!["apple".to_string()],
            replace_start: "écho ".len(),
            suppress_append: true,
            ..Completion::default()
        });
        assert_eq!(buf.contents(), "écho apple");
    }

    #[test]
    fn transpose_words_keeps_the_separator() {
        let mut buf = EditBuffer::new();
        buf.insert_str("one two");
        buf.transpose_words();
        assert_eq!(buf.contents(), "two one");
    }

    #[test]
    fn completion_replacement_uses_utf8_byte_offsets() {
        let mut buf = EditBuffer::new();
        buf.insert_str("λ fo");
        buf.replace_completion(
            &Completion {
                replace_start: "λ ".len(),
                suppress_append: true,
                ..Completion::default()
            },
            "foobar",
        );
        assert_eq!(buf.contents(), "λ foobar");
    }

    #[test]
    fn consecutive_inserts_undo_as_one_edit() {
        let mut buf = EditBuffer::new();
        for ch in "typed".chars() {
            buf.insert(ch);
        }
        buf.break_undo_group();
        buf.undo();
        assert_eq!(buf.contents(), "");
    }
}
