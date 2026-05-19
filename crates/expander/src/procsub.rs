//! Process substitution `<(cmd)` / `>(cmd)`. Delegates to
//! `CommandRunner::spawn_proc_subst` which knows how to wire up a pipe and
//! return a path the consumer can open.

use cherubsh_common::ProcSubstDir;

use crate::buf::ExpandBuf;
use crate::ctx::{ExpCtx, ProcSubstHandle};
use crate::error::ExpandError;

pub fn process_substitute(
    src: &str,
    dir: ProcSubstDir,
    ctx: &mut ExpCtx,
    quoted: bool,
) -> Result<ExpandBuf, ExpandError> {
    let handle: ProcSubstHandle = ctx.runner.spawn_proc_subst(ctx.env, dir, src)?;
    let path = handle.path.clone();
    ctx.proc_subst.push(handle);
    let mut buf = ExpandBuf::with_capacity(path.len());
    if quoted {
        for b in path.bytes() {
            buf.push_quoted(b);
        }
    } else {
        for b in path.bytes() {
            buf.push_literal(b);
        }
    }
    Ok(buf)
}
