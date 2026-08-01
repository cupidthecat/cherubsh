#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_char_conversion_preserves_its_byte_on_signed_and_unsigned_targets() {
        assert_eq!(c_char_byte(b'#' as c_char), b'#');
    }
}
