#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_char_conversion_preserves_its_byte_on_signed_and_unsigned_targets() {
        assert_eq!(c_char_byte(b'#' as c_char), b'#');
    }

    #[test]
    fn history_expand_returns_an_owned_error_for_exponential_input() {
        let input = CString::new(format!("echo {}", "!# ".repeat(18))).unwrap();
        let mut output = ptr::null_mut();

        let status = unsafe { history_expand(input.as_ptr(), &mut output) };

        assert_eq!(status, -1);
        assert!(!output.is_null());
        assert_eq!(
            unsafe { CStr::from_ptr(output) }.to_str().unwrap(),
            "history expansion exceeds the 1 MiB safety limit"
        );
        unsafe { libc::free(output.cast()) };
    }
}
