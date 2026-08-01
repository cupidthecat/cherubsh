/// `<( ... )` / `>( ... )` - invoked by internal.rs which sees the leading
/// `<(` or `>(`.
pub fn process_subst_expand(
    bytes: &[u8],
    i: &mut usize,
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    let dir = if bytes[*i] == b'<' {
        ProcSubstDir::Input
    } else {
        ProcSubstDir::Output
    };
    *i += 2; // skip `<(` or `>(`
    let (body, end) = extract_paren(bytes, *i)?;
    *i = end;
    let src = String::from_utf8_lossy(&body).into_owned();
    let buf = procsub::process_substitute(&src, dir, ctx, quoted)?;
    out.extend_from(&buf);
    Ok(())
}

/// Public for use by internal.rs.
pub fn special_byte(b: u8) -> bool {
    matches!(b, b'?' | b'#' | b'$' | b'!' | b'-' | b'*' | b'@')
}
