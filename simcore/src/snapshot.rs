//! A complete, comparable picture of simulation state at one tick: named
//! per-cell arrays plus named scalars. Both the port and the verification
//! harness produce snapshots, so comparisons are field-by-field and exact.
//!
//! On-disk format (little endian): `b"SIMSNAP1"`, width i32, height i32,
//! tick i32, scalar count u32, per scalar: name (u8 length + bytes) i64;
//! field count u32, per field: name (u8 length + bytes), kind u8, byte
//! length u32, bytes.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    U8 = 1,
    U16 = 2,
    I32 = 3,
    F32 = 4,
}

impl Kind {
    pub fn size(self) -> usize {
        match self {
            Kind::U8 => 1,
            Kind::U16 => 2,
            Kind::I32 | Kind::F32 => 4,
        }
    }
    fn from_u8(v: u8) -> Result<Kind, String> {
        Ok(match v {
            1 => Kind::U8,
            2 => Kind::U16,
            3 => Kind::I32,
            4 => Kind::F32,
            _ => return Err(format!("unknown field kind {v}")),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub kind: Kind,
    pub data: Vec<u8>,
}

impl Field {
    pub fn len(&self) -> usize {
        self.data.len() / self.kind.size()
    }
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    /// Raw element bits (zero-extended) for comparison.
    pub fn bits(&self, i: usize) -> u32 {
        let s = self.kind.size();
        let b = &self.data[i * s..(i + 1) * s];
        match self.kind {
            Kind::U8 => b[0] as u32,
            Kind::U16 => u16::from_le_bytes([b[0], b[1]]) as u32,
            _ => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        }
    }
    pub fn describe(&self, i: usize) -> String {
        let v = self.bits(i);
        match self.kind {
            Kind::F32 => format!("{:?} ({:08x})", f32::from_bits(v), v),
            Kind::I32 => format!("{}", v as i32),
            _ => format!("{v}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub width: i32,
    pub height: i32,
    pub tick: i32,
    pub scalars: Vec<(String, i64)>,
    pub fields: Vec<Field>,
}

pub trait Pod: Copy {
    const KIND: Kind;
    fn write(self, out: &mut Vec<u8>);
}
impl Pod for u8 {
    const KIND: Kind = Kind::U8;
    fn write(self, out: &mut Vec<u8>) {
        out.push(self)
    }
}
impl Pod for u16 {
    const KIND: Kind = Kind::U16;
    fn write(self, out: &mut Vec<u8>) {
        out.extend(self.to_le_bytes())
    }
}
impl Pod for i32 {
    const KIND: Kind = Kind::I32;
    fn write(self, out: &mut Vec<u8>) {
        out.extend(self.to_le_bytes())
    }
}
impl Pod for f32 {
    const KIND: Kind = Kind::F32;
    fn write(self, out: &mut Vec<u8>) {
        out.extend(self.to_bits().to_le_bytes())
    }
}

impl Snapshot {
    pub fn new(width: i32, height: i32, tick: i32) -> Self {
        Snapshot { width, height, tick, ..Default::default() }
    }
    pub fn scalar(&mut self, name: &str, v: i64) {
        self.scalars.push((name.into(), v));
    }
    pub fn field<T: Pod>(&mut self, name: &str, values: &[T]) {
        let mut data = Vec::with_capacity(values.len() * T::KIND.size());
        for v in values {
            v.write(&mut data);
        }
        self.fields.push(Field { name: name.into(), kind: T::KIND, data });
    }
    pub fn raw_field(&mut self, name: &str, kind: Kind, data: Vec<u8>) {
        self.fields.push(Field { name: name.into(), kind, data });
    }
    pub fn get(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = b"SIMSNAP1".to_vec();
        out.extend(self.width.to_le_bytes());
        out.extend(self.height.to_le_bytes());
        out.extend(self.tick.to_le_bytes());
        out.extend((self.scalars.len() as u32).to_le_bytes());
        for (n, v) in &self.scalars {
            out.push(n.len() as u8);
            out.extend_from_slice(n.as_bytes());
            out.extend(v.to_le_bytes());
        }
        out.extend((self.fields.len() as u32).to_le_bytes());
        for f in &self.fields {
            out.push(f.name.len() as u8);
            out.extend_from_slice(f.name.as_bytes());
            out.push(f.kind as u8);
            out.extend((f.data.len() as u32).to_le_bytes());
            out.extend_from_slice(&f.data);
        }
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut r = crate::io::Reader::new(bytes);
        if r.take(8)? != b"SIMSNAP1" {
            return Err("not a snapshot".into());
        }
        let mut s = Snapshot::new(r.i32()?, r.i32()?, r.i32()?);
        let name = |r: &mut crate::io::Reader| -> Result<String, String> {
            let n = r.u8()? as usize;
            String::from_utf8(r.take(n)?.to_vec()).map_err(|_| "invalid name".to_string())
        };
        for _ in 0..r.u32()? {
            let n = name(&mut r)?;
            let v = i64::from_le_bytes(r.take(8)?.try_into().unwrap());
            s.scalars.push((n, v));
        }
        for _ in 0..r.u32()? {
            let n = name(&mut r)?;
            let kind = Kind::from_u8(r.u8()?)?;
            let len = r.u32()? as usize;
            s.fields.push(Field { name: n, kind, data: r.take(len)?.to_vec() });
        }
        Ok(s)
    }
}

/// Result of comparing one field of two snapshots.
#[derive(Debug)]
pub struct FieldDiff {
    pub name: String,
    pub count: usize,
    pub different: usize,
    /// (index, expected, actual) for the first few differences.
    pub first: Vec<(usize, String, String)>,
    /// Largest difference in units in the last place, for f32 fields.
    pub max_ulps: u64,
}

fn ordered(bits: u32) -> i64 {
    // Map f32 bit patterns to a monotonic integer line.
    let b = bits as i32;
    if b < 0 { -((b & 0x7fff_ffff) as i64) } else { b as i64 }
}

/// Compare `actual` against `expected`; fields missing on either side are reported
/// with `different == count == usize::MAX`.
pub fn compare(expected: &Snapshot, actual: &Snapshot) -> (Vec<(String, i64, Option<i64>)>, Vec<FieldDiff>) {
    let scalars = expected
        .scalars
        .iter()
        .map(|(n, v)| (n.clone(), *v, actual.scalars.iter().find(|(m, _)| m == n).map(|x| x.1)))
        .collect();
    let mut out = Vec::new();
    for e in &expected.fields {
        let Some(a) = actual.get(&e.name) else {
            continue;
        };
        let mut d = FieldDiff { name: e.name.clone(), count: e.len(), different: 0, first: Vec::new(), max_ulps: 0 };
        if e.kind != a.kind || e.len() != a.len() {
            d.different = usize::MAX;
            out.push(d);
            continue;
        }
        for i in 0..e.len() {
            let (x, y) = (e.bits(i), a.bits(i));
            if x != y {
                d.different += 1;
                if d.first.len() < 8 {
                    d.first.push((i, e.describe(i), a.describe(i)));
                }
                if e.kind == Kind::F32 {
                    d.max_ulps = d.max_ulps.max(ordered(x).abs_diff(ordered(y)));
                }
            }
        }
        out.push(d);
    }
    (scalars, out)
}

/// 64-bit FNV-1a over bytes.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325_u64;
    for b in bytes {
        h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

impl Snapshot {
    /// Per-field FNV-1a hashes (plus one over all scalars), for cheap
    /// per-tick divergence tracking.
    pub fn field_hashes(&self) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        let mut sc = Vec::new();
        for (n, v) in &self.scalars {
            sc.extend_from_slice(n.as_bytes());
            sc.extend(v.to_le_bytes());
        }
        out.push(("scalars".to_string(), fnv1a(&sc)));
        for f in &self.fields {
            out.push((f.name.clone(), fnv1a(&f.data)));
        }
        out
    }
}
