#![allow(clippy::missing_safety_doc, clippy::vec_box)]

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ffi::{CStr, CString, OsStr};
use std::os::raw::{c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use cherubsh_expander::pattern::{explicitly_matches_dot_name, fnmatch, GlobOpts};

use crate::{Builtin, BuiltinCtx, BuiltinFlags};

const BUILTIN_ENABLED: c_int = 0x01;
const SPECIAL_BUILTIN: c_int = 0x08;
const ASSIGNMENT_BUILTIN: c_int = 0x10;
const POSIX_BUILTIN: c_int = 0x20;
const LOCALVAR_BUILTIN: c_int = 0x40;

type BuiltinFunction = unsafe extern "C" fn(*mut WordList) -> c_int;
type LoadFunction = unsafe extern "C" fn(*const c_char) -> c_int;
type UnloadFunction = unsafe extern "C" fn(*const c_char);

#[repr(C)]
struct WordDesc {
    word: *mut c_char,
    flags: c_int,
}

#[repr(C)]
struct WordList {
    next: *mut WordList,
    word: *mut WordDesc,
}

#[repr(C)]
struct BashBuiltin {
    name: *const c_char,
    function: Option<BuiltinFunction>,
    flags: c_int,
    long_doc: *const *const c_char,
    short_doc: *const c_char,
    handle: *mut c_char,
}

#[repr(C)]
struct ShellVariable {
    name: *mut c_char,
    value: *mut c_char,
    exportstr: *mut c_char,
    dynamic_value: *mut c_void,
    assign_func: *mut c_void,
    attributes: c_int,
    context: c_int,
}

#[repr(C)]
struct ArrayElementState {
    kind: i16,
    subtype: i16,
    index: i64,
    key: *mut c_char,
    value: *mut c_char,
}

#[repr(C)]
struct BashArray {
    max_index: i64,
    num_elements: i64,
    head: *mut BashArrayElement,
    lastref: *mut BashArrayElement,
}

#[repr(C)]
struct BashArrayElement {
    index: i64,
    value: *mut c_char,
    next: *mut BashArrayElement,
    previous: *mut BashArrayElement,
}

#[repr(C)]
struct BashHashTable {
    buckets: *mut *mut BashHashBucket,
    bucket_count: c_int,
    entry_count: c_int,
}

#[repr(C)]
struct BashHashBucket {
    next: *mut BashHashBucket,
    key: *mut c_char,
    data: *mut c_void,
    hash: u32,
    times_found: c_int,
}

struct AbiArrayEntry {
    element: Box<BashArrayElement>,
    _value: CString,
}

struct AbiArray {
    array: Box<BashArray>,
    head: Box<BashArrayElement>,
    entries: Vec<AbiArrayEntry>,
}

struct AbiAssocEntry {
    bucket: Box<BashHashBucket>,
    _key: CString,
    _value: CString,
}

struct AbiAssoc {
    table: Box<BashHashTable>,
    buckets: Vec<*mut BashHashBucket>,
    entries: Vec<AbiAssocEntry>,
}

enum AbiVariableValue {
    Scalar(Option<CString>),
    Indexed(AbiArray),
    Assoc(AbiAssoc),
}

struct AbiVariable {
    variable: Box<ShellVariable>,
    name: CString,
    value: AbiVariableValue,
    attrs: cherubsh_common::VarAttrs,
    dirty: bool,
}

#[derive(Default)]
struct AbiVariableStore {
    variables: Vec<Box<AbiVariable>>,
}

fn abi_cstring(value: impl AsRef<str>) -> CString {
    CString::new(value.as_ref().replace('\0', "")).expect("removed null bytes")
}

impl AbiArray {
    fn new(values: Vec<(i64, String)>) -> Self {
        let mut array = Self {
            array: Box::new(BashArray {
                max_index: -1,
                num_elements: 0,
                head: ptr::null_mut(),
                lastref: ptr::null_mut(),
            }),
            head: Box::new(BashArrayElement {
                index: -1,
                value: ptr::null_mut(),
                next: ptr::null_mut(),
                previous: ptr::null_mut(),
            }),
            entries: Vec::new(),
        };
        array.replace(values);
        array
    }

    fn replace(&mut self, mut values: Vec<(i64, String)>) {
        values.sort_by_key(|(index, _)| *index);
        self.entries = values
            .into_iter()
            .map(|(index, value)| {
                let value = abi_cstring(value);
                let element = Box::new(BashArrayElement {
                    index,
                    value: value.as_ptr().cast_mut(),
                    next: ptr::null_mut(),
                    previous: ptr::null_mut(),
                });
                AbiArrayEntry {
                    element,
                    _value: value,
                }
            })
            .collect();
        self.relink();
    }

    fn relink(&mut self) {
        let head = (&mut *self.head) as *mut BashArrayElement;
        let pointers = self
            .entries
            .iter_mut()
            .map(|entry| (&mut *entry.element) as *mut BashArrayElement)
            .collect::<Vec<_>>();
        if pointers.is_empty() {
            self.head.next = head;
            self.head.previous = head;
        } else {
            self.head.next = pointers[0];
            self.head.previous = *pointers.last().expect("nonempty pointers");
            for (index, pointer) in pointers.iter().copied().enumerate() {
                unsafe {
                    (*pointer).previous = if index == 0 {
                        head
                    } else {
                        pointers[index - 1]
                    };
                    (*pointer).next = pointers.get(index + 1).copied().unwrap_or(head);
                }
            }
        }
        self.array.max_index = self
            .entries
            .iter()
            .map(|entry| entry.element.index)
            .max()
            .unwrap_or(-1);
        self.array.num_elements = self.entries.len() as i64;
        self.array.head = head;
        self.array.lastref = head;
    }

    fn values_from_links(&self) -> Vec<(i64, String)> {
        let head = (&*self.head) as *const BashArrayElement as *mut BashArrayElement;
        let mut current = self.head.next;
        let mut values = Vec::new();
        let limit = self.entries.len().saturating_add(1);
        while !current.is_null() && current != head && values.len() < limit {
            unsafe {
                let value = if (*current).value.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr((*current).value)
                        .to_string_lossy()
                        .into_owned()
                };
                values.push(((*current).index, value));
                current = (*current).next;
            }
        }
        values
    }
}

fn bash_hash(value: &[u8]) -> u32 {
    value.iter().fold(0u32, |hash, byte| {
        hash.wrapping_mul(16_777_619) ^ u32::from(*byte)
    })
}

impl AbiAssoc {
    fn new(values: Vec<(String, String)>) -> Self {
        let mut assoc = Self {
            table: Box::new(BashHashTable {
                buckets: ptr::null_mut(),
                bucket_count: 128,
                entry_count: 0,
            }),
            buckets: vec![ptr::null_mut(); 128],
            entries: Vec::new(),
        };
        assoc.replace(values);
        assoc
    }

    fn replace(&mut self, values: Vec<(String, String)>) {
        self.entries = values
            .into_iter()
            .map(|(key, value)| {
                let key = abi_cstring(key);
                let value = abi_cstring(value);
                let bucket = Box::new(BashHashBucket {
                    next: ptr::null_mut(),
                    key: key.as_ptr().cast_mut(),
                    data: value.as_ptr().cast_mut().cast(),
                    hash: bash_hash(key.as_bytes()),
                    times_found: 0,
                });
                AbiAssocEntry {
                    bucket,
                    _key: key,
                    _value: value,
                }
            })
            .collect();
        self.relink();
    }

    fn relink(&mut self) {
        self.buckets.fill(ptr::null_mut());
        for entry in &mut self.entries {
            let index = entry.bucket.hash as usize % self.buckets.len();
            entry.bucket.next = self.buckets[index];
            self.buckets[index] = (&mut *entry.bucket) as *mut BashHashBucket;
        }
        self.table.buckets = self.buckets.as_mut_ptr();
        self.table.bucket_count = self.buckets.len() as c_int;
        self.table.entry_count = self.entries.len() as c_int;
    }

    fn values_from_buckets(&self) -> Vec<(String, String)> {
        let mut values = Vec::new();
        for first in &self.buckets {
            let mut current = *first;
            let mut remaining = self.entries.len().saturating_add(1);
            while !current.is_null() && remaining > 0 {
                unsafe {
                    let key = CStr::from_ptr((*current).key)
                        .to_string_lossy()
                        .into_owned();
                    let value = CStr::from_ptr((*current).data.cast())
                        .to_string_lossy()
                        .into_owned();
                    values.push((key, value));
                    current = (*current).next;
                }
                remaining -= 1;
            }
        }
        values
    }
}

fn bash_variable_attributes(
    kind: cherubsh_common::VarKind,
    attrs: cherubsh_common::VarAttrs,
) -> c_int {
    let mut flags = 0;
    if attrs.contains(cherubsh_common::VarAttrs::EXPORT) {
        flags |= 0x0001;
    }
    if attrs.contains(cherubsh_common::VarAttrs::READONLY) {
        flags |= 0x0002;
    }
    if kind == cherubsh_common::VarKind::Indexed {
        flags |= 0x0004;
    }
    if attrs.contains(cherubsh_common::VarAttrs::INTEGER) {
        flags |= 0x0010;
    }
    if attrs.contains(cherubsh_common::VarAttrs::LOCAL) {
        flags |= 0x0020;
    }
    if kind == cherubsh_common::VarKind::Assoc {
        flags |= 0x0040;
    }
    if attrs.contains(cherubsh_common::VarAttrs::TRACE) {
        flags |= 0x0080;
    }
    if attrs.contains(cherubsh_common::VarAttrs::UPPERCASE) {
        flags |= 0x0100;
    }
    if attrs.contains(cherubsh_common::VarAttrs::LOWERCASE) {
        flags |= 0x0200;
    }
    if attrs.contains(cherubsh_common::VarAttrs::CAPCASE) {
        flags |= 0x0400;
    }
    if attrs.contains(cherubsh_common::VarAttrs::NAMEREF) {
        flags |= 0x0800;
    }
    flags
}

fn common_variable_attributes(flags: c_int) -> cherubsh_common::VarAttrs {
    let mut attrs = cherubsh_common::VarAttrs::empty();
    for (mask, attr) in [
        (0x0001, cherubsh_common::VarAttrs::EXPORT),
        (0x0002, cherubsh_common::VarAttrs::READONLY),
        (0x0010, cherubsh_common::VarAttrs::INTEGER),
        (0x0020, cherubsh_common::VarAttrs::LOCAL),
        (0x0080, cherubsh_common::VarAttrs::TRACE),
        (0x0100, cherubsh_common::VarAttrs::UPPERCASE),
        (0x0200, cherubsh_common::VarAttrs::LOWERCASE),
        (0x0400, cherubsh_common::VarAttrs::CAPCASE),
        (0x0800, cherubsh_common::VarAttrs::NAMEREF),
    ] {
        if flags & mask != 0 {
            attrs |= attr;
        }
    }
    attrs
}

impl AbiVariable {
    fn from_snapshot(snapshot: cherubsh_common::VarSnapshot) -> Box<Self> {
        let name = abi_cstring(&snapshot.name);
        let value = match snapshot.kind {
            cherubsh_common::VarKind::Indexed => {
                AbiVariableValue::Indexed(AbiArray::new(snapshot.indexed.unwrap_or_default()))
            }
            cherubsh_common::VarKind::Assoc => {
                AbiVariableValue::Assoc(AbiAssoc::new(snapshot.assoc.unwrap_or_default()))
            }
            _ => AbiVariableValue::Scalar(snapshot.scalar.map(abi_cstring)),
        };
        let mut variable = Box::new(Self {
            variable: Box::new(ShellVariable {
                name: ptr::null_mut(),
                value: ptr::null_mut(),
                exportstr: ptr::null_mut(),
                dynamic_value: ptr::null_mut(),
                assign_func: ptr::null_mut(),
                attributes: bash_variable_attributes(snapshot.kind, snapshot.attrs),
                context: 0,
            }),
            name,
            value,
            attrs: snapshot.attrs,
            dirty: false,
        });
        variable.refresh_pointers();
        variable
    }

    fn empty(name: &str, kind: cherubsh_common::VarKind) -> Box<Self> {
        Self::from_snapshot(cherubsh_common::VarSnapshot {
            name: name.to_string(),
            kind,
            attrs: cherubsh_common::VarAttrs::empty(),
            scalar: (kind == cherubsh_common::VarKind::Scalar).then(String::new),
            indexed: (kind == cherubsh_common::VarKind::Indexed).then(Vec::new),
            assoc: (kind == cherubsh_common::VarKind::Assoc).then(Vec::new),
            nameref_target: None,
        })
    }

    fn refresh_pointers(&mut self) {
        self.variable.name = self.name.as_ptr().cast_mut();
        self.variable.value = match &mut self.value {
            AbiVariableValue::Scalar(value) => value
                .as_ref()
                .map_or(ptr::null_mut(), |value| value.as_ptr().cast_mut()),
            AbiVariableValue::Indexed(array) => {
                (&mut *array.array as *mut BashArray).cast::<c_char>()
            }
            AbiVariableValue::Assoc(assoc) => {
                (&mut *assoc.table as *mut BashHashTable).cast::<c_char>()
            }
        };
    }
}

pub struct DynamicBuiltin {
    name: &'static str,
    synopsis: &'static str,
    abi_name: CString,
    abi_synopsis: CString,
    symbol_name: CString,
    function: BuiltinFunction,
    bash_flags: c_int,
    long_doc: *const *const c_char,
    handle: *mut c_void,
    active: AtomicBool,
}

unsafe impl Send for DynamicBuiltin {}
unsafe impl Sync for DynamicBuiltin {}

thread_local! {
    static ABI_CONTEXT: Cell<*mut c_void> = const { Cell::new(ptr::null_mut()) };
    static ABI_EXPORT_STRINGS: RefCell<Vec<CString>> = const { RefCell::new(Vec::new()) };
    static ABI_EXPORT_POINTERS: RefCell<Vec<*mut c_char>> = const { RefCell::new(Vec::new()) };
    static ABI_IFS: RefCell<CString> = RefCell::new(CString::new(" \t\n").expect("default IFS"));
    static ABI_VARIABLES: RefCell<AbiVariableStore> = RefCell::new(AbiVariableStore::default());
    static ABI_SCRATCH: RefCell<Vec<CString>> = const { RefCell::new(Vec::new()) };
}

struct AbiContextGuard {
    previous: *mut c_void,
}

impl AbiContextGuard {
    fn enter(ctx: &mut BuiltinCtx<'_>) -> Self {
        let pointer = (ctx as *mut BuiltinCtx<'_>).cast::<c_void>();
        let previous = ABI_CONTEXT.with(|slot| slot.replace(pointer));
        if previous.is_null() {
            ABI_VARIABLES.with(|store| store.borrow_mut().variables.clear());
            ABI_SCRATCH.with(|scratch| scratch.borrow_mut().clear());
        }
        Self { previous }
    }
}

impl Drop for AbiContextGuard {
    fn drop(&mut self) {
        if self.previous.is_null() {
            ABI_CONTEXT.with(|slot| {
                let pointer = slot.get();
                if !pointer.is_null() {
                    unsafe {
                        sync_abi_variables(&mut *pointer.cast::<BuiltinCtx<'_>>());
                    }
                }
            });
            ABI_VARIABLES.with(|store| store.borrow_mut().variables.clear());
            ABI_SCRATCH.with(|scratch| scratch.borrow_mut().clear());
        }
        ABI_CONTEXT.with(|slot| slot.set(self.previous));
    }
}

fn with_abi_context<R>(fallback: R, operation: impl FnOnce(&mut BuiltinCtx<'_>) -> R) -> R {
    ABI_CONTEXT.with(|slot| {
        let pointer = slot.get();
        if pointer.is_null() {
            fallback
        } else {
            unsafe { operation(&mut *pointer.cast::<BuiltinCtx<'_>>()) }
        }
    })
}

fn sync_abi_variables(ctx: &mut BuiltinCtx<'_>) {
    ABI_VARIABLES.with(|store| {
        let store = store.borrow();
        for variable in &store.variables {
            if !variable.dirty {
                continue;
            }
            let name = variable.name.to_string_lossy();
            match &variable.value {
                AbiVariableValue::Scalar(_) => {
                    if !variable.attrs.contains(cherubsh_common::VarAttrs::READONLY) {
                        if !variable.variable.dynamic_value.is_null() {
                            let dynamic: unsafe extern "C" fn(
                                *mut ShellVariable,
                            )
                                -> *mut ShellVariable =
                                unsafe { std::mem::transmute(variable.variable.dynamic_value) };
                            unsafe {
                                dynamic((&*variable.variable as *const ShellVariable).cast_mut());
                            }
                        }
                        if !variable.variable.value.is_null() {
                            let value = unsafe { CStr::from_ptr(variable.variable.value) }
                                .to_string_lossy()
                                .into_owned();
                            let _ = ctx.env().assign(&name, value);
                        } else {
                            ctx.env().unset(&name);
                        }
                        restore_variable_attributes(
                            ctx.env(),
                            &name,
                            common_variable_attributes(variable.variable.attributes),
                        );
                    }
                }
                AbiVariableValue::Indexed(array) => {
                    ctx.env().set_array(&name, Vec::new());
                    for (index, value) in array.values_from_links() {
                        ctx.env().set_array_indexed(&name, index, value);
                    }
                    restore_variable_attributes(
                        ctx.env(),
                        &name,
                        common_variable_attributes(variable.variable.attributes),
                    );
                }
                AbiVariableValue::Assoc(assoc) => {
                    ctx.env().unset(&name);
                    for (key, value) in assoc.values_from_buckets() {
                        ctx.env().set_array_assoc(&name, &key, value);
                    }
                    restore_variable_attributes(
                        ctx.env(),
                        &name,
                        common_variable_attributes(variable.variable.attributes),
                    );
                }
            }
        }
    });
}

fn restore_variable_attributes(
    env: &mut dyn cherubsh_common::Environment,
    name: &str,
    attrs: cherubsh_common::VarAttrs,
) {
    for attr in [
        cherubsh_common::VarAttrs::EXPORT,
        cherubsh_common::VarAttrs::READONLY,
        cherubsh_common::VarAttrs::INTEGER,
        cherubsh_common::VarAttrs::UPPERCASE,
        cherubsh_common::VarAttrs::LOWERCASE,
        cherubsh_common::VarAttrs::CAPCASE,
        cherubsh_common::VarAttrs::TRACE,
        cherubsh_common::VarAttrs::NAMEREF,
        cherubsh_common::VarAttrs::LOCAL,
    ] {
        env.set_attr(name, attr, attrs.contains(attr));
    }
}

fn c_text(pointer: *const c_char) -> Option<String> {
    if pointer.is_null() {
        None
    } else {
        Some(
            unsafe { CStr::from_ptr(pointer) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

unsafe fn run_native_builtin_from_word_list(
    builtin: &'static dyn Builtin,
    mut list: *mut WordList,
) -> c_int {
    let mut args = Vec::new();
    let mut flags = Vec::new();
    while !list.is_null() {
        let word = (*list).word;
        if word.is_null() || (*word).word.is_null() {
            args.push(String::new());
            flags.push(0);
        } else {
            args.push(CStr::from_ptr((*word).word).to_string_lossy().into_owned());
            flags.push((*word).flags as u32);
        }
        list = (*list).next;
    }
    with_abi_context(1, |ctx| {
        let mut nested = BuiltinCtx {
            args: &args,
            arg_flags: &flags,
            assignments: &[],
            redirects: &[],
            invoked_via_command: false,
            shell: ctx.shell,
        };
        builtin.run(&mut nested)
    })
}

#[no_mangle]
unsafe extern "C" fn printf_builtin(list: *mut WordList) -> c_int {
    run_native_builtin_from_word_list(&crate::printf::PRINTF, list)
}

#[no_mangle]
unsafe extern "C" fn cd_builtin(list: *mut WordList) -> c_int {
    run_native_builtin_from_word_list(&crate::cd::CD, list)
}

fn abi_variable_pointer(
    name: &str,
    create: Option<cherubsh_common::VarKind>,
    dirty: bool,
) -> *mut ShellVariable {
    with_abi_context(ptr::null_mut(), |ctx| {
        let snapshot = ctx.env_ref().var_snapshot(name);
        let missing = snapshot.is_none();
        ABI_VARIABLES.with(|store| {
            let mut store = store.borrow_mut();
            if let Some(variable) = store
                .variables
                .iter_mut()
                .find(|variable| variable.name.to_bytes() == name.as_bytes())
            {
                variable.dirty |= dirty;
                return (&mut *variable.variable) as *mut ShellVariable;
            }
            let variable = match snapshot {
                Some(snapshot) => AbiVariable::from_snapshot(snapshot),
                None => match create {
                    Some(kind) => AbiVariable::empty(name, kind),
                    None => return ptr::null_mut(),
                },
            };
            store.variables.push(variable);
            let variable = store.variables.last_mut().expect("inserted ABI variable");
            variable.dirty = dirty || missing;
            (&mut *variable.variable) as *mut ShellVariable
        })
    })
}

fn variable_for_array(
    pointer: *mut BashArray,
    operation: impl FnOnce(&mut AbiVariable, &mut AbiArray),
) -> bool {
    ABI_VARIABLES.with(|store| {
        let mut store = store.borrow_mut();
        for variable in &mut store.variables {
            let variable_pointer = (&mut **variable) as *mut AbiVariable;
            unsafe {
                let AbiVariableValue::Indexed(array) = &mut (*variable_pointer).value else {
                    continue;
                };
                if std::ptr::eq(&*array.array, pointer) {
                    let array_pointer = array as *mut AbiArray;
                    operation(&mut *variable_pointer, &mut *array_pointer);
                    return true;
                }
            }
        }
        false
    })
}

fn variable_for_assoc(
    pointer: *mut BashHashTable,
    operation: impl FnOnce(&mut AbiVariable, &mut AbiAssoc),
) -> bool {
    ABI_VARIABLES.with(|store| {
        let mut store = store.borrow_mut();
        for variable in &mut store.variables {
            let variable_pointer = (&mut **variable) as *mut AbiVariable;
            unsafe {
                let AbiVariableValue::Assoc(assoc) = &mut (*variable_pointer).value else {
                    continue;
                };
                if std::ptr::eq(&*assoc.table, pointer) {
                    let assoc_pointer = assoc as *mut AbiAssoc;
                    operation(&mut *variable_pointer, &mut *assoc_pointer);
                    return true;
                }
            }
        }
        false
    })
}

#[no_mangle]
unsafe extern "C" fn find_variable(name: *const c_char) -> *mut ShellVariable {
    c_text(name).map_or(ptr::null_mut(), |name| {
        abi_variable_pointer(&name, None, true)
    })
}

#[no_mangle]
unsafe extern "C" fn find_variable_last_nameref(
    name: *const c_char,
    _flags: c_int,
) -> *mut ShellVariable {
    find_variable(name)
}

#[no_mangle]
unsafe extern "C" fn bind_variable(
    name: *const c_char,
    value: *const c_char,
    _flags: c_int,
) -> *mut ShellVariable {
    let Some(name) = c_text(name) else {
        return ptr::null_mut();
    };
    let value = c_text(value);
    if let Some(value) = &value {
        with_abi_context((), |ctx| {
            let _ = ctx.env().assign(&name, value.clone());
        });
    }
    let pointer = abi_variable_pointer(&name, Some(cherubsh_common::VarKind::Scalar), true);
    if !pointer.is_null() {
        ABI_VARIABLES.with(|store| {
            let mut store = store.borrow_mut();
            if let Some(variable) = store
                .variables
                .iter_mut()
                .find(|variable| std::ptr::eq(&*variable.variable, pointer))
            {
                variable.value = AbiVariableValue::Scalar(value.map(abi_cstring));
                variable.refresh_pointers();
                variable.dirty = true;
            }
        });
    }
    pointer
}

#[no_mangle]
unsafe extern "C" fn builtin_bind_variable(
    name: *mut c_char,
    value: *mut c_char,
    flags: c_int,
) -> *mut ShellVariable {
    if let (Some(reference), Some(value_text)) = (c_text(name), c_text(value)) {
        if let Some((base, subscript)) = reference
            .split_once('[')
            .and_then(|(base, rest)| rest.strip_suffix(']').map(|key| (base, key)))
        {
            let kind = with_abi_context(cherubsh_common::VarKind::Scalar, |ctx| {
                ctx.env_ref().kind(base)
            });
            let variable = abi_variable_pointer(base, None, true);
            if !variable.is_null() {
                let c_value = abi_cstring(&value_text);
                match kind {
                    cherubsh_common::VarKind::Indexed => {
                        let index = subscript.parse::<i64>().unwrap_or(0);
                        return bind_array_element(
                            variable,
                            index,
                            c_value.as_ptr().cast_mut(),
                            flags,
                        );
                    }
                    cherubsh_common::VarKind::Assoc => {
                        let c_base = abi_cstring(base);
                        let c_key = abi_cstring(subscript);
                        return bind_assoc_variable(
                            variable,
                            c_base.as_ptr(),
                            c_key.as_ptr().cast_mut(),
                            c_value.as_ptr(),
                            flags,
                        );
                    }
                    _ => {}
                }
            }
        }
    }
    bind_variable(name, value, flags)
}

#[no_mangle]
unsafe extern "C" fn unbind_variable(name: *const c_char) -> c_int {
    let Some(name) = c_text(name) else {
        return -1;
    };
    with_abi_context((), |ctx| ctx.env().unset(&name));
    ABI_VARIABLES.with(|store| {
        store
            .borrow_mut()
            .variables
            .retain(|variable| variable.name.to_bytes() != name.as_bytes());
    });
    0
}

#[no_mangle]
unsafe extern "C" fn builtin_unbind_variable(name: *const c_char) -> c_int {
    unbind_variable(name)
}

#[no_mangle]
unsafe extern "C" fn get_variable_value(variable: *mut ShellVariable) -> *mut c_char {
    if variable.is_null() {
        ptr::null_mut()
    } else {
        (*variable).value
    }
}

#[no_mangle]
unsafe extern "C" fn get_string_value(name: *const c_char) -> *mut c_char {
    let variable = find_variable(name);
    get_variable_value(variable)
}

#[no_mangle]
unsafe extern "C" fn find_or_make_array_variable(
    name: *const c_char,
    flags: c_int,
) -> *mut ShellVariable {
    c_text(name).map_or(ptr::null_mut(), |name| {
        let kind = if flags & 2 != 0 {
            cherubsh_common::VarKind::Assoc
        } else {
            cherubsh_common::VarKind::Indexed
        };
        abi_variable_pointer(&name, Some(kind), true)
    })
}

#[no_mangle]
unsafe extern "C" fn builtin_find_indexed_array(
    name: *mut c_char,
    flags: c_int,
) -> *mut ShellVariable {
    c_text(name).map_or(ptr::null_mut(), |name| {
        abi_variable_pointer(
            &name,
            (flags != 0).then_some(cherubsh_common::VarKind::Indexed),
            true,
        )
    })
}

#[no_mangle]
unsafe extern "C" fn array_insert(array: *mut BashArray, index: i64, value: *mut c_char) -> c_int {
    let value = c_text(value).unwrap_or_default();
    if variable_for_array(array, |variable, array| {
        let mut values = array
            .values_from_links()
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        values.insert(index, value);
        array.replace(values.into_iter().collect());
        variable.dirty = true;
        variable.refresh_pointers();
    }) {
        0
    } else {
        -1
    }
}

#[no_mangle]
unsafe extern "C" fn array_flush(array: *mut BashArray) {
    variable_for_array(array, |variable, array| {
        array.replace(Vec::new());
        variable.dirty = true;
        variable.refresh_pointers();
    });
}

#[no_mangle]
unsafe extern "C" fn bind_array_element(
    variable: *mut ShellVariable,
    index: i64,
    value: *mut c_char,
    _flags: c_int,
) -> *mut ShellVariable {
    if variable.is_null() {
        return ptr::null_mut();
    }
    let array = (*variable).value.cast::<BashArray>();
    if array_insert(array, index, value) == 0 {
        variable
    } else {
        ptr::null_mut()
    }
}

#[no_mangle]
unsafe extern "C" fn bind_assoc_variable(
    variable: *mut ShellVariable,
    _name: *const c_char,
    key: *mut c_char,
    value: *const c_char,
    _flags: c_int,
) -> *mut ShellVariable {
    if variable.is_null() {
        return ptr::null_mut();
    }
    let key = c_text(key).unwrap_or_default();
    let value = c_text(value).unwrap_or_default();
    let table = (*variable).value.cast::<BashHashTable>();
    if variable_for_assoc(table, |variable, assoc| {
        let mut values = assoc
            .values_from_buckets()
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        values.insert(key, value);
        assoc.replace(values.into_iter().collect());
        variable.dirty = true;
        variable.refresh_pointers();
    }) {
        variable
    } else {
        ptr::null_mut()
    }
}

#[no_mangle]
unsafe extern "C" fn assoc_flush(table: *mut BashHashTable) {
    variable_for_assoc(table, |variable, assoc| {
        assoc.replace(Vec::new());
        variable.dirty = true;
        variable.refresh_pointers();
    });
}

#[no_mangle]
unsafe extern "C" fn array_variable_name(
    reference: *const c_char,
    _flags: c_int,
    subscript: *mut *mut c_char,
    length: *mut c_int,
) -> *mut c_char {
    if reference.is_null() {
        return ptr::null_mut();
    }
    let bytes = CStr::from_ptr(reference).to_bytes();
    let end = bytes
        .iter()
        .position(|byte| *byte == b'[')
        .unwrap_or(bytes.len());
    let result = libc::malloc(end + 1).cast::<c_char>();
    if result.is_null() {
        return result;
    }
    ptr::copy_nonoverlapping(reference, result, end);
    *result.add(end) = 0;
    if !subscript.is_null() {
        *subscript = if end < bytes.len() {
            reference.add(end + 1).cast_mut()
        } else {
            ptr::null_mut()
        };
    }
    if !length.is_null() {
        *length = end as c_int;
    }
    result
}

#[no_mangle]
unsafe extern "C" fn array_variable_part(
    reference: *const c_char,
    flags: c_int,
    subscript: *mut *mut c_char,
    length: *mut c_int,
) -> *mut ShellVariable {
    let name = array_variable_name(reference, flags, subscript, length);
    if name.is_null() {
        return ptr::null_mut();
    }
    let variable = find_variable(name);
    libc::free(name.cast());
    variable
}

#[no_mangle]
unsafe extern "C" fn get_array_value(
    reference: *const c_char,
    _flags: c_int,
    state: *mut c_void,
) -> *mut c_char {
    let Some(reference) = c_text(reference) else {
        return ptr::null_mut();
    };
    let (name, key) = reference
        .split_once('[')
        .map(|(name, key)| (name, key.trim_end_matches(']')))
        .unwrap_or((&reference, "0"));
    let value = with_abi_context(None, |ctx| match ctx.env_ref().kind(name) {
        cherubsh_common::VarKind::Indexed => {
            let index = key.parse::<i64>().unwrap_or(0);
            if !state.is_null() {
                let state = state.cast::<ArrayElementState>();
                (*state).kind = 1;
                (*state).index = index;
            }
            ctx.env_ref().get_array_indexed(name, index)
        }
        cherubsh_common::VarKind::Assoc => {
            if !state.is_null() {
                (*state.cast::<ArrayElementState>()).kind = 2;
            }
            ctx.env_ref().get_array_assoc(name, key)
        }
        _ => ctx.env_ref().get(name),
    });
    let Some(value) = value else {
        return ptr::null_mut();
    };
    ABI_SCRATCH.with(|scratch| {
        let mut scratch = scratch.borrow_mut();
        scratch.push(abi_cstring(value));
        scratch
            .last()
            .expect("inserted scratch value")
            .as_ptr()
            .cast_mut()
    })
}

#[no_mangle]
pub extern "C" fn cherub_abi_last_status() -> c_int {
    with_abi_context(1, |ctx| ctx.env_ref().last_status())
}

#[no_mangle]
pub extern "C" fn cherub_abi_set_status(status: c_int) {
    with_abi_context((), |ctx| ctx.env().set_last_status(status));
}

#[no_mangle]
pub unsafe extern "C" fn cherub_abi_force_variable(
    name: *const c_char,
    value: *const c_char,
    attributes: c_int,
) {
    let (Some(name), Some(value)) = (c_text(name), c_text(value)) else {
        return;
    };
    with_abi_context((), |ctx| {
        ctx.env()
            .set_attr(&name, cherubsh_common::VarAttrs::READONLY, false);
        ctx.env().set(&name, value);
        restore_variable_attributes(ctx.env(), &name, common_variable_attributes(attributes));
    });
}

#[no_mangle]
pub unsafe extern "C" fn cherub_abi_run_source(
    source: *const c_char,
    requested_exit: *mut c_int,
) -> c_int {
    if !requested_exit.is_null() {
        *requested_exit = -1;
    }
    let Some(source) = c_text(source) else {
        return 2;
    };
    with_abi_context(2, |ctx| {
        let status = ctx.shell.run_source(&source);
        if !requested_exit.is_null() {
            if let Some(exit_status) = ctx.shell.requested_exit_status() {
                unsafe {
                    *requested_exit = exit_status;
                }
            }
        }
        status
    })
}

#[no_mangle]
pub extern "C" fn cherub_abi_next_input_line() -> *mut c_char {
    with_abi_context(ptr::null_mut(), |ctx| {
        let Some(line) = ctx.env().next_shell_input_line() else {
            return ptr::null_mut();
        };
        ABI_SCRATCH.with(|scratch| {
            let mut scratch = scratch.borrow_mut();
            scratch.push(abi_cstring(line));
            scratch
                .last()
                .expect("inserted input line")
                .as_ptr()
                .cast_mut()
        })
    })
}

#[no_mangle]
pub extern "C" fn cherub_abi_enter_loadable_child() {
    with_abi_context((), |ctx| ctx.env().enter_loadable_child());
}

#[no_mangle]
pub unsafe extern "C" fn cherub_abi_source_complete(source: *const c_char) -> c_int {
    let Some(source) = c_text(source) else {
        return 1;
    };
    with_abi_context(1, |ctx| source_is_complete(&source, ctx.env_ref()) as c_int)
}

fn source_is_complete(source: &str, env: &dyn cherubsh_common::Environment) -> bool {
    if source_has_open_quote(source)
        || source_has_line_continuation(source)
        || source_has_pending_heredoc(source)
    {
        return false;
    }
    let parse_source = cherubsh_common::expand_aliases_for_parse(source, env);
    let mut lexer = cherubsh_lexer::Lexer::new(&parse_source);
    lexer.set_extglob_patterns(env.option("extglob"));
    lexer.set_posix_mode(env.option("posix"));
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token() {
        tokens.push(token);
    }
    let mut parser = cherubsh_parser::Parser::new(tokens, &parse_source);
    match parser.parse_input_unit() {
        Ok(_) => true,
        Err(error) => {
            let at_end = error
                .span
                .as_ref()
                .map(|span| span.end >= parse_source.len())
                .unwrap_or(true);
            !(at_end
                && (error.message.starts_with("expected")
                    || error.message == "function body must be a compound command"
                    || error.message == "syntax error: unexpected end of file"))
        }
    }
}

fn source_has_open_quote(source: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for byte in source.bytes() {
        if escaped {
            escaped = false;
            continue;
        }
        match quote {
            Some(b'\'') => {
                if byte == b'\'' {
                    quote = None;
                }
            }
            Some(b'"') => {
                if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    quote = None;
                }
            }
            Some(b'`') => {
                if byte == b'\\' {
                    escaped = true;
                } else if byte == b'`' {
                    quote = None;
                }
            }
            _ if byte == b'\\' => escaped = true,
            _ if matches!(byte, b'\'' | b'"' | b'`') => quote = Some(byte),
            _ => {}
        }
    }
    quote.is_some()
}

fn source_has_line_continuation(source: &str) -> bool {
    let line = source.strip_suffix('\n').unwrap_or(source);
    line.bytes().rev().take_while(|byte| *byte == b'\\').count() % 2 == 1
}

fn source_has_pending_heredoc(source: &str) -> bool {
    let mut pending = std::collections::VecDeque::<(String, bool)>::new();
    for line in source.lines() {
        if let Some((delimiter, strip_tabs)) = pending.front() {
            let candidate = if *strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            if candidate == delimiter {
                pending.pop_front();
            }
            continue;
        }
        pending.extend(heredoc_delimiters(line));
    }
    !pending.is_empty()
}

fn heredoc_delimiters(line: &str) -> Vec<(String, bool)> {
    let bytes = line.as_bytes();
    let mut result = Vec::new();
    let mut index = 0;
    let mut quote = None;
    while index + 1 < bytes.len() {
        let byte = bytes[index];
        if let Some(active) = quote {
            if byte == active {
                quote = None;
            } else if byte == b'\\' && active != b'\'' {
                index += 1;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
            index += 1;
            continue;
        }
        if byte != b'<' || bytes[index + 1] != b'<' || bytes.get(index + 2) == Some(&b'<') {
            index += 1;
            continue;
        }
        index += 2;
        let strip_tabs = bytes.get(index) == Some(&b'-');
        if strip_tabs {
            index += 1;
        }
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        let mut delimiter = String::new();
        let mut delimiter_quote = None;
        while let Some(&current) = bytes.get(index) {
            if let Some(active) = delimiter_quote {
                if current == active {
                    delimiter_quote = None;
                } else {
                    delimiter.push(current as char);
                }
            } else if matches!(current, b'\'' | b'"') {
                delimiter_quote = Some(current);
            } else if current.is_ascii_whitespace()
                || matches!(current, b';' | b'&' | b'|' | b'<' | b'>')
            {
                break;
            } else if current == b'\\' {
                index += 1;
                if let Some(&escaped) = bytes.get(index) {
                    delimiter.push(escaped as char);
                }
            } else {
                delimiter.push(current as char);
            }
            index += 1;
        }
        if !delimiter.is_empty() {
            result.push((delimiter, strip_tabs));
        }
    }
    result
}

#[no_mangle]
pub extern "C" fn cherub_abi_export_environment() -> *mut *mut c_char {
    with_abi_context(ptr::null_mut(), |ctx| {
        ABI_EXPORT_STRINGS.with(|storage| {
            let mut storage = storage.borrow_mut();
            storage.clear();
            for variable in ctx.env_ref().iter_vars() {
                if !variable.attrs.contains(cherubsh_common::VarAttrs::EXPORT) {
                    continue;
                }
                let value = variable.scalar.unwrap_or_default();
                if let Ok(entry) = CString::new(format!("{}={value}", variable.name)) {
                    storage.push(entry);
                }
            }
            ABI_EXPORT_POINTERS.with(|pointers| {
                let mut pointers = pointers.borrow_mut();
                pointers.clear();
                pointers.extend(storage.iter().map(|entry| entry.as_ptr().cast_mut()));
                pointers.push(ptr::null_mut());
                pointers.as_mut_ptr()
            })
        })
    })
}

#[no_mangle]
pub extern "C" fn cherub_abi_ifs() -> *mut c_char {
    with_abi_context(ptr::null_mut(), |ctx| {
        let value = ctx.env_ref().ifs_raw();
        ABI_IFS.with(|storage| {
            let mut storage = storage.borrow_mut();
            *storage = CString::new(value).unwrap_or_else(|_| CString::default());
            storage.as_ptr().cast_mut()
        })
    })
}

impl Builtin for DynamicBuiltin {
    fn name(&self) -> &'static str {
        self.name
    }

    fn flags(&self) -> BuiltinFlags {
        flags_from_bash(self.bash_flags)
    }

    fn synopsis(&self) -> &'static str {
        self.synopsis
    }

    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        if !self.active.load(Ordering::Acquire) {
            return 1;
        }
        let mut strings = Vec::with_capacity(ctx.args.len());
        for argument in ctx.args {
            let Ok(value) = CString::new(argument.as_bytes()) else {
                eprintln!("cherubsh: {}: argument contains a null byte", self.name);
                return 1;
            };
            strings.push(value);
        }
        let mut words = strings
            .iter_mut()
            .enumerate()
            .map(|(index, value)| WordDesc {
                word: value.as_ptr().cast_mut(),
                flags: ctx.arg_flag(index) as c_int,
            })
            .collect::<Vec<_>>();
        let mut list = (0..words.len())
            .map(|_| WordList {
                next: ptr::null_mut(),
                word: ptr::null_mut(),
            })
            .collect::<Vec<_>>();
        for index in 0..list.len() {
            list[index].word = &mut words[index];
            if index + 1 < list.len() {
                list[index].next = &mut list[index + 1];
            }
        }
        let head = list.first_mut().map_or(ptr::null_mut(), |item| item);
        let _guard = AbiContextGuard::enter(ctx);
        let status = unsafe {
            cherub_invoke_builtin(
                self.function,
                head,
                self.abi_name.as_ptr(),
                self.abi_synopsis.as_ptr(),
                self.long_doc,
            )
        };
        status.rem_euclid(256)
    }
}

#[derive(Default)]
struct Registry {
    entries: Vec<&'static DynamicBuiltin>,
}

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

pub fn lookup(name: &str) -> Option<&'static dyn Builtin> {
    let registry = registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    registry
        .entries
        .iter()
        .rev()
        .copied()
        .find(|entry| entry.name == name && entry.active.load(Ordering::Acquire))
        .map(|entry| entry as &'static dyn Builtin)
}

pub fn iter() -> Vec<&'static dyn Builtin> {
    let registry = registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let mut seen = std::collections::HashSet::new();
    registry
        .entries
        .iter()
        .rev()
        .copied()
        .filter(|entry| entry.active.load(Ordering::Acquire) && seen.insert(entry.name))
        .map(|entry| entry as &'static dyn Builtin)
        .collect()
}

pub fn is_dynamic(name: &str) -> bool {
    let registry = registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    registry
        .entries
        .iter()
        .rev()
        .any(|entry| entry.name == name && entry.active.load(Ordering::Acquire))
}

pub fn load(
    ctx: &mut BuiltinCtx<'_>,
    filename: &str,
    requested_names: &[String],
    special: bool,
    disabled: bool,
) -> Result<(), String> {
    if requested_names.is_empty() {
        return Err("no builtin names supplied".to_string());
    }
    let path = resolve_load_path(ctx, filename)?;
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| format!("{}: invalid shared object path", path.display()))?;
    clear_dlerror();
    let handle = unsafe { dlopen(c_path.as_ptr(), libc::RTLD_LAZY) };
    if handle.is_null() {
        return Err(format!(
            "cannot open shared object {}: {}",
            filename,
            dlerror_message()
        ));
    }

    let mut loaded = Vec::new();
    let mut errors = Vec::new();
    for requested in requested_names {
        if requested.contains('/') {
            errors.push(format!(
                "{requested}: builtin names may not contain slashes"
            ));
            continue;
        }
        let struct_symbol = format!("{requested}_struct");
        let Ok(c_symbol) = CString::new(struct_symbol.as_bytes()) else {
            errors.push(format!(
                "cannot find {struct_symbol} in shared object {filename}"
            ));
            continue;
        };
        clear_dlerror();
        let raw = unsafe { dlsym(handle, c_symbol.as_ptr()) };
        if raw.is_null() {
            errors.push(format!(
                "cannot find {struct_symbol} in shared object {filename}: {}",
                dlerror_message()
            ));
            continue;
        }
        let descriptor = unsafe { &mut *raw.cast::<BashBuiltin>() };
        let Some(function) = descriptor.function else {
            errors.push(format!(
                "{requested}: shared object has no builtin function"
            ));
            continue;
        };

        let load_symbol = format!("{requested}_builtin_load");
        if let Some(load_hook) = unsafe { symbol::<LoadFunction>(handle, &load_symbol) } {
            let c_name = CString::new(requested.as_bytes()).expect("validated builtin name");
            let _guard = AbiContextGuard::enter(ctx);
            if unsafe { cherub_invoke_load(load_hook, c_name.as_ptr()) } == 0 {
                errors.push(format!(
                    "load function for {requested} returns failure (0): not loaded"
                ));
                continue;
            }
        }

        let descriptor_name = unsafe { optional_c_string(descriptor.name) }
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| requested.clone());
        let synopsis = unsafe { optional_c_string(descriptor.short_doc) }.unwrap_or_default();
        let leaked_name = Box::leak(descriptor_name.into_boxed_str());
        let leaked_synopsis = Box::leak(synopsis.into_boxed_str());
        let mut flags = descriptor.flags;
        flags &= !0x04;
        if special {
            flags |= SPECIAL_BUILTIN;
        }
        if disabled {
            flags &= !BUILTIN_ENABLED;
        }
        descriptor.flags = flags;
        descriptor.handle = handle.cast();
        let entry: &'static DynamicBuiltin = Box::leak(Box::new(DynamicBuiltin {
            name: leaked_name,
            synopsis: leaked_synopsis,
            abi_name: CString::new(leaked_name.as_bytes()).expect("loadable builtin name"),
            abi_synopsis: CString::new(leaked_synopsis.as_bytes()).unwrap_or_default(),
            symbol_name: CString::new(requested.as_bytes()).expect("loadable symbol name"),
            function,
            bash_flags: flags,
            long_doc: descriptor.long_doc,
            handle,
            active: AtomicBool::new(true),
        }));
        loaded.push(entry);
    }

    if loaded.is_empty() {
        unsafe {
            dlclose(handle);
        }
        return Err(errors.join("\n"));
    }

    {
        let mut registry = registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        for entry in &loaded {
            for old in registry.entries.iter().rev() {
                if old.name == entry.name && old.active.swap(false, Ordering::AcqRel) {
                    break;
                }
            }
            registry.entries.push(*entry);
        }
    }
    for entry in loaded {
        ctx.env().builtin_set_enabled(entry.name, !disabled);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

pub fn unload(ctx: &mut BuiltinCtx<'_>, name: &str) -> Result<(), String> {
    let entry = {
        let registry = registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        registry
            .entries
            .iter()
            .rev()
            .copied()
            .find(|entry| entry.name == name && entry.active.load(Ordering::Acquire))
    }
    .ok_or_else(|| format!("{name}: not dynamically loaded"))?;

    let symbol_name = entry.symbol_name.to_string_lossy();
    let unload_symbol = format!("{symbol_name}_builtin_unload");
    if let Some(unload_hook) = unsafe { symbol::<UnloadFunction>(entry.handle, &unload_symbol) } {
        let c_name = CString::new(symbol_name.as_bytes()).expect("validated builtin name");
        let _guard = AbiContextGuard::enter(ctx);
        unsafe { cherub_invoke_unload(unload_hook, c_name.as_ptr()) };
    }
    entry.active.store(false, Ordering::Release);
    ctx.env().builtin_set_enabled(name, false);

    let last_reference = {
        let registry = registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        !registry.entries.iter().any(|candidate| {
            candidate.handle == entry.handle && candidate.active.load(Ordering::Acquire)
        })
    };
    if last_reference && unsafe { dlclose(entry.handle) } != 0 {
        return Err(format!("{name}: cannot delete: {}", dlerror_message()));
    }
    Ok(())
}

fn resolve_load_path(ctx: &BuiltinCtx<'_>, filename: &str) -> Result<PathBuf, String> {
    let path = Path::new(filename);
    if filename.contains('/') {
        return Ok(path.to_path_buf());
    }
    if let Some(search) = ctx.env_ref().get("BASH_LOADABLES_PATH") {
        for directory in search.split(':') {
            let directory = if directory.is_empty() { "." } else { directory };
            let candidate = Path::new(directory).join(filename);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        return Err(format!("{filename}: file not found in BASH_LOADABLES_PATH"));
    }
    Ok(Path::new(".").join(filename))
}

fn flags_from_bash(flags: c_int) -> BuiltinFlags {
    let mut result = BuiltinFlags::empty();
    if flags & SPECIAL_BUILTIN != 0 {
        result |= BuiltinFlags::SPECIAL;
    }
    if flags & ASSIGNMENT_BUILTIN != 0 {
        result |= BuiltinFlags::ASSIGNMENT;
    }
    if flags & POSIX_BUILTIN != 0 {
        result |= BuiltinFlags::POSIX;
    }
    if flags & LOCALVAR_BUILTIN != 0 {
        result |= BuiltinFlags::LOCALVAR;
    }
    result
}

unsafe fn optional_c_string(pointer: *const c_char) -> Option<String> {
    (!pointer.is_null()).then(|| CStr::from_ptr(pointer).to_string_lossy().into_owned())
}

unsafe fn symbol<T: Copy>(handle: *mut c_void, name: &str) -> Option<T> {
    let name = CString::new(name.as_bytes()).ok()?;
    clear_dlerror();
    let pointer = dlsym(handle, name.as_ptr());
    if pointer.is_null() {
        None
    } else {
        Some(std::mem::transmute_copy(&pointer))
    }
}

fn clear_dlerror() {
    unsafe {
        dlerror();
    }
}

fn dlerror_message() -> String {
    let pointer = unsafe { dlerror() };
    if pointer.is_null() {
        "unknown dynamic loader error".to_string()
    } else {
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    }
}

/// Bash's loadable test builtin resolves this matcher from the shell's
/// exported symbol table. A zero result means that the pattern matched.
#[no_mangle]
pub unsafe extern "C" fn strmatch(
    pattern: *const c_char,
    string: *const c_char,
    flags: c_int,
) -> c_int {
    if pattern.is_null() || string.is_null() {
        return 1;
    }
    let pattern = CStr::from_ptr(pattern).to_bytes();
    let string = CStr::from_ptr(string).to_bytes();
    let options = GlobOpts {
        nocaseglob: flags & (1 << 4) != 0,
        extglob: flags & (1 << 5) != 0,
        globasciiranges: true,
    };
    let pathname = flags & (1 << 0) != 0;
    let period = flags & (1 << 2) != 0;
    let leading_dir = flags & (1 << 3) != 0;

    let matched = if pathname {
        let patterns = split_path_pattern(pattern);
        let strings = string.split(|byte| *byte == b'/').collect::<Vec<_>>();
        if patterns.len() > strings.len() || (!leading_dir && patterns.len() != strings.len()) {
            false
        } else {
            patterns
                .iter()
                .zip(strings.iter())
                .all(|(pattern, string)| match_segment(pattern, string, options, period))
        }
    } else {
        match_segment(pattern, string, options, period)
    };
    if matched {
        0
    } else {
        1
    }
}

fn split_path_pattern(pattern: &[u8]) -> Vec<Vec<u8>> {
    let mut segments = vec![Vec::new()];
    let mut index = 0;
    while index < pattern.len() {
        if pattern[index] == b'\\' && pattern.get(index + 1) == Some(&b'/') {
            segments.push(Vec::new());
            index += 2;
        } else if pattern[index] == b'/' {
            segments.push(Vec::new());
            index += 1;
        } else {
            segments
                .last_mut()
                .expect("initial segment")
                .push(pattern[index]);
            index += 1;
        }
    }
    segments
}

fn match_segment(pattern: &[u8], string: &[u8], options: GlobOpts, period: bool) -> bool {
    if period
        && string.first() == Some(&b'.')
        && !explicitly_matches_dot_name(pattern, string, options)
    {
        return false;
    }
    fnmatch(pattern, string, options)
}

#[link(name = "dl")]
unsafe extern "C" {
    fn cherub_invoke_builtin(
        function: BuiltinFunction,
        list: *mut WordList,
        name: *const c_char,
        synopsis: *const c_char,
        help: *const *const c_char,
    ) -> c_int;
    fn cherub_invoke_load(function: LoadFunction, name: *const c_char) -> c_int;
    fn cherub_invoke_unload(function: UnloadFunction, name: *const c_char);
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
    fn dlerror() -> *const c_char;
}

#[allow(dead_code)]
fn _assert_os_str(_: &OsStr) {}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    #[test]
    fn exported_strmatch_handles_posix_equivalence_classes() {
        let pattern = CString::new("[[=b=]a]").unwrap();
        for value in ["a", "b"] {
            let value = CString::new(value).unwrap();
            let status = unsafe { super::strmatch(pattern.as_ptr(), value.as_ptr(), 1 << 5) };
            assert_eq!(status, 0, "{value:?}");
        }

        let malformed = CString::new("[[=]=]ab]").unwrap();
        let value = CString::new("a").unwrap();
        let status = unsafe { super::strmatch(malformed.as_ptr(), value.as_ptr(), 1 << 5) };
        assert_eq!(status, 1);
    }
}
