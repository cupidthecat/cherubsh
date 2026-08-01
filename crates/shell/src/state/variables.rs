#[derive(Debug, Default, Clone)]
pub struct VariableEntry {
    pub value: String,
    pub has_value: bool,
    pub exported: bool,
    pub readonly: bool,
    pub attrs: VarAttrs,
}
