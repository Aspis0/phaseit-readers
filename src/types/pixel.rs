/// Per-sample type. Extend `size_bytes`, [`PlaneBuf`] and the readers together when adding one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DType {
    U8,
    U16,
    I16,
    F32,
}

impl DType {
    pub fn size_bytes(&self) -> usize {
        match self {
            DType::U8 => 1,
            DType::U16 => 2,
            DType::I16 => 2,
            DType::F32 => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}

/// The host's byte order: bytes already in it are read as-is, so there is one decoder, not two.
pub(super) fn host_endian() -> Endian {
    if cfg!(target_endian = "little") {
        Endian::Little
    } else {
        Endian::Big
    }
}
