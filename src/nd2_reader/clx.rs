
/// Unhandled kinds (bytearrays) are kept opaque.
#[allow(dead_code)] // general-purpose tree; not every kind/getter is used by the MVP
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ClxValue {
    Bool(bool),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(String),
    Bytes(Vec<u8>),
    Level(ClxMap),
    List(Vec<ClxValue>),
}

#[allow(dead_code)]
impl ClxValue {
    fn as_u64(&self) -> Option<u64> {
        match self {
            ClxValue::Bool(b) => Some(*b as u64),
            ClxValue::I32(v) => u64::try_from(*v).ok(),
            ClxValue::U32(v) => Some(*v as u64),
            ClxValue::I64(v) => u64::try_from(*v).ok(),
            ClxValue::U64(v) => Some(*v),
            ClxValue::F64(v) if *v >= 0.0 => Some(*v as u64),
            _ => None,
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            ClxValue::Bool(b) => Some(*b as u64 as f64),
            ClxValue::I32(v) => Some(*v as f64),
            ClxValue::U32(v) => Some(*v as f64),
            ClxValue::I64(v) => Some(*v as f64),
            ClxValue::U64(v) => Some(*v as f64),
            ClxValue::F64(v) => Some(*v),
            _ => None,
        }
    }

    pub(super) fn as_str(&self) -> Option<&str> {
        match self {
            ClxValue::Str(s) => Some(s),
            _ => None,
        }
    }

    pub(super) fn as_level(&self) -> Option<&ClxMap> {
        match self {
            ClxValue::Level(m) => Some(m),
            _ => None,
        }
    }

    /// Truthy in the LIM sense (used for `pItemValid` flags).
    pub(super) fn is_truthy(&self) -> bool {
        self.as_u64().map(|v| v != 0).unwrap_or(false)
    }
}

/// One CLX LEVEL: repeated empty names collect into a list, as the reference format uses them for list items.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ClxMap {
    pub(super) entries: Vec<(String, ClxValue)>,
}

#[allow(dead_code)]
impl ClxMap {
    pub(super) fn new() -> Self {
        ClxMap { entries: Vec::new() }
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn get(&self, key: &str) -> Option<&ClxValue> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub(super) fn get_level(&self, key: &str) -> Option<&ClxMap> {
        self.get(key).and_then(ClxValue::as_level)
    }

    pub(super) fn get_u32(&self, key: &str) -> Option<u32> {
        self.get(key).and_then(ClxValue::as_u64).and_then(|v| u32::try_from(v).ok())
    }

    pub(super) fn get_usize(&self, key: &str) -> Option<usize> {
        self.get(key).and_then(ClxValue::as_u64).and_then(|v| usize::try_from(v).ok())
    }

    pub(super) fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(|v| match v {
            ClxValue::Bool(b) => Some(*b as i64),
            ClxValue::I32(x) => Some(*x as i64),
            ClxValue::U32(x) => Some(*x as i64),
            ClxValue::I64(x) => Some(*x),
            ClxValue::U64(x) => i64::try_from(*x).ok(),
            ClxValue::F64(x) => Some(*x as i64),
            _ => None,
        })
    }

    pub(super) fn get_f64(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(ClxValue::as_f64)
    }

    pub(super) fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(ClxValue::as_str)
    }

    /// A repeated non-empty name overwrites (last wins); a repeated empty name collects into a list.
    pub(super) fn insert(&mut self, name: String, value: ClxValue) {
        if name.is_empty() {
            if let Some(slot) = self.entries.iter_mut().find(|(k, _)| k.is_empty()) {
                match &mut slot.1 {
                    ClxValue::List(v) => v.push(value),
                    _ => {
                        let old = std::mem::replace(&mut slot.1, ClxValue::List(Vec::new()));
                        if let ClxValue::List(v) = &mut slot.1 {
                            v.push(old);
                            v.push(value);
                        }
                    }
                }
                return;
            }
        } else if let Some(slot) = self.entries.iter_mut().find(|(k, _)| *k == name) {
            slot.1 = value;
            return;
        }
        self.entries.push((name, value));
    }
}

/// CLX record type ids (`ELxLiteVariantType`).
pub(super) mod clx_type {
    pub const BOOL: u8 = 1;
    pub const INT32: u8 = 2;
    pub const UINT32: u8 = 3;
    pub const INT64: u8 = 4;
    pub const UINT64: u8 = 5;
    pub const DOUBLE: u8 = 6;
    pub const VOIDPOINTER: u8 = 7;
    pub const STRING: u8 = 8;
    pub const BYTEARRAY: u8 = 9;
    pub const LEVEL: u8 = 11;
    pub const COMPRESS: u8 = 76;
}

/// Mirrors the reference's `if len(val) == 1 and "" in val`: a lone empty-name entry is a list.
pub(super) fn collapse_level(mut child: ClxMap) -> ClxValue {
    if child.entries.len() == 1 && child.entries[0].0.is_empty() {
        match child.entries.pop().unwrap().1 {
            ClxValue::List(v) => ClxValue::List(v),
            single => ClxValue::List(vec![single]),
        }
    } else {
        ClxValue::Level(child)
    }
}
