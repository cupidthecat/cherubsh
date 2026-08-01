#![no_main]

use cherubsh_lexer::Lexer;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let data = &data[..data.len().min(64 * 1024)];
    let Some((&options, input)) = data.split_first() else {
        return;
    };
    let Ok(input) = std::str::from_utf8(input) else {
        return;
    };
    let mut lexer = Lexer::new(input);
    lexer.set_extglob_patterns(options & 1 != 0);
    lexer.set_posix_mode(options & 2 != 0);
    lexer.set_comments_enabled(options & 4 == 0);
    while lexer.next_token().is_some() {}
});
