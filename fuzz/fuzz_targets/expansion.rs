#![no_main]

use cherubsh_expander::{brace::brace_expand, quote};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let data = &data[..data.len().min(4096)];
    let decoded = quote::ansi_c_decode(data);
    let quoted = quote::shell_quote(&decoded);
    let _ = quote::ansi_c_decode(&quoted);

    let mut list_input = Vec::new();
    let mut opening_braces = 0;
    let mut commas = 0;
    const LIST_ALPHABET: &[u8] = b"abc{},\\'\"_-";
    for byte in data.iter().take(64) {
        let mut mapped = LIST_ALPHABET[usize::from(*byte) % LIST_ALPHABET.len()];
        if mapped == b'{' {
            opening_braces += 1;
            if opening_braces > 4 {
                mapped = b'a';
            }
        } else if mapped == b',' {
            commas += 1;
            if commas > 8 {
                mapped = b'b';
            }
        }
        list_input.push(mapped);
    }
    let _ = brace_expand(&list_input);

    let start = data.first().copied().unwrap_or(0) % 64;
    let end = data.get(1).copied().unwrap_or(0) % 64;
    let step = data.get(2).copied().unwrap_or(1) % 8 + 1;
    let sequence = format!("{{{start}..{end}..{step}}}");
    let _ = brace_expand(sequence.as_bytes());
});
