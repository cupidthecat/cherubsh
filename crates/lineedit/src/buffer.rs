//! Line buffer with cursor + undo.

#[derive(Default)]
pub struct EditBuffer {
    chars: Vec<char>,
    point: usize,
    undo_stack: Vec<(Vec<char>, usize)>,
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

    fn byte_point(&self) -> usize {
        self.chars[..self.point].iter().map(|c| c.len_utf8()).sum()
    }

    fn snapshot(&mut self) {
        self.undo_stack.push((self.chars.clone(), self.point));
        if self.undo_stack.len() > 128 {
            self.undo_stack.remove(0);
        }
    }

    pub fn undo(&mut self) {
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
        self.snapshot();
        self.chars.insert(self.point, c);
        self.point += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        self.snapshot();
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

    pub fn move_word_left(&mut self) {
        while self.point > 0 && !self.chars[self.point - 1].is_alphanumeric() {
            self.point -= 1;
        }
        while self.point > 0 && self.chars[self.point - 1].is_alphanumeric() {
            self.point -= 1;
        }
    }

    pub fn move_word_right(&mut self) {
        while self.point < self.chars.len() && !self.chars[self.point].is_alphanumeric() {
            self.point += 1;
        }
        while self.point < self.chars.len() && self.chars[self.point].is_alphanumeric() {
            self.point += 1;
        }
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

    pub fn apply_completion(&mut self, matches: &[String]) {
        if matches.is_empty() {
            return;
        }
        // Replace the word under cursor.
        let word_start = self.find_word_start();
        let cur_word: String = self.chars[word_start..self.point].iter().collect();
        let common = common_prefix(matches);
        let chosen = if matches.len() == 1 {
            matches[0].clone()
        } else if common.len() > cur_word.len() {
            common
        } else {
            // Print all matches on a fresh line via bell - caller's render will recover.
            use std::io::Write;
            let _ = writeln!(std::io::stderr());
            for m in matches {
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
        if matches.len() == 1 {
            self.chars.insert(self.point, ' ');
            self.point += 1;
        }
    }

    fn find_word_start(&self) -> usize {
        let mut i = self.point;
        while i > 0 && !is_word_break(self.chars[i - 1]) {
            i -= 1;
        }
        i
    }
}

fn is_word_break(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\n' | '|' | '&' | ';' | '<' | '>' | '(' | ')'
    )
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

    #[test]
    fn completion_preserves_tail_after_cursor() {
        let mut buf = EditBuffer::new();
        buf.insert_str("echo appl suffix");
        for _ in 0.." suffix".len() {
            buf.move_left();
        }
        buf.apply_completion(&["apple".to_string()]);
        assert_eq!(buf.contents(), "echo apple  suffix");
    }
}
