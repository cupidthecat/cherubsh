use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Colon;
pub static COLON: Colon = Colon;
impl Builtin for Colon {
    fn name(&self) -> &'static str {
        ":"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        ": [arguments]"
    }
    fn run(&self, _ctx: &mut BuiltinCtx<'_>) -> i32 {
        0
    }
}

pub struct True;
pub static TRUE: True = True;
impl Builtin for True {
    fn name(&self) -> &'static str {
        "true"
    }
    fn run(&self, _ctx: &mut BuiltinCtx<'_>) -> i32 {
        0
    }
}

pub struct False;
pub static FALSE: False = False;
impl Builtin for False {
    fn name(&self) -> &'static str {
        "false"
    }
    fn run(&self, _ctx: &mut BuiltinCtx<'_>) -> i32 {
        1
    }
}
