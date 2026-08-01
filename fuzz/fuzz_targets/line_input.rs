#![no_main]

use cherubsh_lineedit::InputDecoder;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let data = &data[..data.len().min(64 * 1024)];
    let mut decoder = InputDecoder::new(data.first().is_some_and(|byte| byte & 1 != 0));
    let mut offset = usize::from(!data.is_empty());
    let mut chunk_index = 0usize;
    while offset < data.len() {
        let selector = data.get(chunk_index).copied().unwrap_or(1);
        let chunk_len = usize::from(selector % 16 + 1);
        let end = (offset + chunk_len).min(data.len());
        for event in decoder.push(&data[offset..end]) {
            let _ = event.to_sequence();
        }
        offset = end;
        chunk_index = chunk_index.saturating_add(1);
    }
    for event in decoder.finish() {
        let _ = event.to_sequence();
    }
});
