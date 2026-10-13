//! JSON text equal to `serde_json::to_string(&serde_json::to_value(x))`,
//! written without building the `Value` tree. Object keys come out sorted
//! (serde_json's `Map` is a `BTreeMap` here) and `f32` is printed as the `f64`
//! it widens to, as `Value` stores it. Leaves are formatted by serde_json
//! itself.
//!
//! A large cluster is a few hundred MB of JSON; building it as a `Value` costs
//! millions of small allocations, which was most of the WASM run time and
//! memory.
use std::borrow::Cow;
use std::fmt::Display;

use serde::ser::{self, Serialize};

#[derive(Debug)]
pub struct Error(String);
impl Display for Error { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str(&self.0) } }
impl std::error::Error for Error {}
impl ser::Error for Error { fn custom<T: Display>(msg: T) -> Self { Error(msg.to_string()) } }
type R<T = ()> = Result<T, Error>;

pub fn to_vec(value: &(impl Serialize + ?Sized)) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    value.serialize(Ser(&mut out)).map_err(|e| e.0)?;
    Ok(out)
}

/// A JSON object assembled field by field; [`finish`](Self::finish) sorts the
/// keys, so the text matches serializing the same fields through `Value`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Object { fields: Vec<(String, Vec<u8>)> }

impl Object {
    /// Insert or replace `key`.
    pub fn set(&mut self, key: &str, value: &(impl Serialize + ?Sized)) -> Result<(), String> {
        let text = to_vec(value)?;
        self.set_raw(key, text);
        Ok(())
    }

    /// Insert or replace `key` with already canonical JSON text.
    pub fn set_raw(&mut self, key: &str, text: Vec<u8>) {
        match self.fields.iter_mut().find(|(k, _)| k == key) {
            Some(field) => field.1 = text,
            None => self.fields.push((key.to_string(), text)),
        }
    }

    /// The JSON text of `key`, if set.
    pub fn get(&self, key: &str) -> Option<&[u8]> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_slice())
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.fields.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out = Vec::with_capacity(self.fields.iter().map(|(k, v)| k.len() + v.len() + 4).sum::<usize>() + 2);
        out.push(b'{');
        for (i, (k, v)) in self.fields.iter().enumerate() {
            if i > 0 { out.push(b','); }
            serde_json::to_writer(&mut out, k).expect("string key");
            out.push(b':');
            out.extend_from_slice(v);
        }
        out.push(b'}');
        out
    }
}

fn leaf(out: &mut Vec<u8>, value: &(impl Serialize + ?Sized)) -> R {
    serde_json::to_writer(out, value).map_err(|e| Error(e.to_string()))
}

struct Ser<'a>(&'a mut Vec<u8>);

/// Array writer, optionally wrapped as `{"Variant":[...]}`.
struct Seq<'a> { out: &'a mut Vec<u8>, first: bool, variant: bool }

impl Seq<'_> {
    fn element(&mut self, value: &(impl Serialize + ?Sized)) -> R {
        if !self.first { self.out.push(b','); }
        self.first = false;
        value.serialize(Ser(self.out))
    }
    fn finish(self) -> R {
        self.out.push(b']');
        if self.variant { self.out.push(b'}'); }
        Ok(())
    }
}

/// Object writer: values are rendered into one scratch buffer and copied out in
/// key order at the end. Duplicate keys keep the last value, as `Map::insert`.
struct Obj<'a> { out: &'a mut Vec<u8>, buf: Vec<u8>, fields: Vec<(Cow<'static, str>, usize, usize)>, key: Option<String>, variant: bool }

impl<'a> Obj<'a> {
    fn new(out: &'a mut Vec<u8>, variant: Option<&'static str>) -> R<Self> {
        if let Some(v) = variant { variant_open(out, v)?; }
        Ok(Obj { out, buf: Vec::new(), fields: Vec::new(), key: None, variant: variant.is_some() })
    }
    fn field(&mut self, key: Cow<'static, str>, value: &(impl Serialize + ?Sized)) -> R {
        let start = self.buf.len();
        value.serialize(Ser(&mut self.buf))?;
        self.fields.push((key, start, self.buf.len()));
        Ok(())
    }
    fn finish(mut self) -> R {
        self.fields.sort_by(|a, b| a.0.cmp(&b.0));
        self.out.push(b'{');
        let mut first = true;
        for i in 0..self.fields.len() {
            if self.fields.get(i + 1).is_some_and(|next| next.0 == self.fields[i].0) { continue; }
            let (key, start, end) = &self.fields[i];
            if !first { self.out.push(b','); }
            first = false;
            leaf(self.out, key.as_ref())?;
            self.out.push(b':');
            self.out.extend_from_slice(&self.buf[*start..*end]);
        }
        self.out.push(b'}');
        if self.variant { self.out.push(b'}'); }
        Ok(())
    }
}

fn variant_open(out: &mut Vec<u8>, variant: &str) -> R {
    out.push(b'{');
    leaf(out, variant)?;
    out.push(b':');
    Ok(())
}

impl<'a> ser::Serializer for Ser<'a> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Seq<'a>;
    type SerializeTuple = Seq<'a>;
    type SerializeTupleStruct = Seq<'a>;
    type SerializeTupleVariant = Seq<'a>;
    type SerializeMap = Obj<'a>;
    type SerializeStruct = Obj<'a>;
    type SerializeStructVariant = Obj<'a>;

    fn serialize_bool(self, v: bool) -> R { leaf(self.0, &v) }
    fn serialize_i8(self, v: i8) -> R { leaf(self.0, &v) }
    fn serialize_i16(self, v: i16) -> R { leaf(self.0, &v) }
    fn serialize_i32(self, v: i32) -> R { leaf(self.0, &v) }
    fn serialize_i64(self, v: i64) -> R { leaf(self.0, &v) }
    fn serialize_i128(self, v: i128) -> R { leaf(self.0, &serde_json::to_value(v).map_err(|e| Error(e.to_string()))?) }
    fn serialize_u8(self, v: u8) -> R { leaf(self.0, &v) }
    fn serialize_u16(self, v: u16) -> R { leaf(self.0, &v) }
    fn serialize_u32(self, v: u32) -> R { leaf(self.0, &v) }
    fn serialize_u64(self, v: u64) -> R { leaf(self.0, &v) }
    fn serialize_u128(self, v: u128) -> R { leaf(self.0, &serde_json::to_value(v).map_err(|e| Error(e.to_string()))?) }
    // `Value` widens f32 to f64; non-finite numbers become null either way.
    fn serialize_f32(self, v: f32) -> R { leaf(self.0, &(v as f64)) }
    fn serialize_f64(self, v: f64) -> R { leaf(self.0, &v) }
    fn serialize_char(self, v: char) -> R { leaf(self.0, &v) }
    fn serialize_str(self, v: &str) -> R { leaf(self.0, v) }
    fn serialize_bytes(self, v: &[u8]) -> R { leaf(self.0, v) }
    fn serialize_none(self) -> R { self.0.extend_from_slice(b"null"); Ok(()) }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> R { v.serialize(self) }
    fn serialize_unit(self) -> R { self.serialize_none() }
    fn serialize_unit_struct(self, _: &'static str) -> R { self.serialize_none() }
    fn serialize_unit_variant(self, _: &'static str, _: u32, variant: &'static str) -> R { leaf(self.0, variant) }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _: &'static str, v: &T) -> R { v.serialize(self) }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(self, _: &'static str, _: u32, variant: &'static str, v: &T) -> R {
        variant_open(self.0, variant)?;
        v.serialize(Ser(self.0))?;
        self.0.push(b'}');
        Ok(())
    }
    fn serialize_seq(self, _: Option<usize>) -> R<Seq<'a>> { self.0.push(b'['); Ok(Seq { out: self.0, first: true, variant: false }) }
    fn serialize_tuple(self, len: usize) -> R<Seq<'a>> { self.serialize_seq(Some(len)) }
    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> R<Seq<'a>> { self.serialize_seq(Some(len)) }
    fn serialize_tuple_variant(self, _: &'static str, _: u32, variant: &'static str, _: usize) -> R<Seq<'a>> {
        variant_open(self.0, variant)?;
        self.0.push(b'[');
        Ok(Seq { out: self.0, first: true, variant: true })
    }
    fn serialize_map(self, _: Option<usize>) -> R<Obj<'a>> { Obj::new(self.0, None) }
    fn serialize_struct(self, _: &'static str, _: usize) -> R<Obj<'a>> { Obj::new(self.0, None) }
    fn serialize_struct_variant(self, _: &'static str, _: u32, variant: &'static str, _: usize) -> R<Obj<'a>> { Obj::new(self.0, Some(variant)) }
}

impl ser::SerializeSeq for Seq<'_> {
    type Ok = (); type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> R { self.element(v) }
    fn end(self) -> R { self.finish() }
}
impl ser::SerializeTuple for Seq<'_> {
    type Ok = (); type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> R { self.element(v) }
    fn end(self) -> R { self.finish() }
}
impl ser::SerializeTupleStruct for Seq<'_> {
    type Ok = (); type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> R { self.element(v) }
    fn end(self) -> R { self.finish() }
}
impl ser::SerializeTupleVariant for Seq<'_> {
    type Ok = (); type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> R { self.element(v) }
    fn end(self) -> R { self.finish() }
}
impl ser::SerializeMap for Obj<'_> {
    type Ok = (); type Error = Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> R {
        // serde_json's MapKeySerializer: strings, chars, unit variants and
        // newtypes as text; integers and bools by their decimal text.
        let key = match serde_json::to_value(key).map_err(|e| Error(e.to_string()))? {
            serde_json::Value::String(s) => s,
            serde_json::Value::Number(n) if n.is_i64() || n.is_u64() => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            _ => return Err(Error("key must be a string".into())),
        };
        self.key = Some(key);
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> R {
        let key = self.key.take().ok_or_else(|| Error("value without key".into()))?;
        self.field(Cow::Owned(key), v)
    }
    fn end(self) -> R { self.finish() }
}
impl ser::SerializeStruct for Obj<'_> {
    type Ok = (); type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, v: &T) -> R { self.field(Cow::Borrowed(key), v) }
    fn end(self) -> R { self.finish() }
}
impl ser::SerializeStructVariant for Obj<'_> {
    type Ok = (); type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, v: &T) -> R { self.field(Cow::Borrowed(key), v) }
    fn end(self) -> R { self.finish() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};

    #[derive(serde::Serialize)]
    enum E { Unit, New(f32), Tuple(i32, Option<u8>), Struct { z: bool, a: Vec<f64> } }

    #[derive(serde::Serialize)]
    struct S {
        zeta: f32, alpha: Vec<E>, map: HashMap<i32, &'static str>, tuple: (i64, u64, char), nested: BTreeMap<String, S2>,
        text: String, none: Option<()>, nan: f32, big: f64, yaml: serde_yaml::Value,
        #[serde(flatten)] extra: BTreeMap<String, i32>,
    }
    #[derive(serde::Serialize)]
    struct S2 { b: (), a: [u8; 2] }

    #[test]
    fn matches_serializing_through_value() {
        let yaml: serde_yaml::Value = serde_yaml::from_str("{z: 1, a: [x, {q: 2.5, b: null}], m: !tag 3}").unwrap();
        let s = S {
            zeta: 0.1, alpha: vec![E::Unit, E::New(1.5e-7), E::Tuple(-3, None), E::Struct { z: true, a: vec![1.0, -0.0, 1e300] }],
            map: HashMap::from([(10, "ten"), (2, "two"), (-1, "minus")]), tuple: (i64::MIN, u64::MAX, '"'),
            nested: BTreeMap::from([("k\n\u{1}".into(), S2 { b: (), a: [1, 2] })]), text: "é\\/\"".into(), none: None,
            nan: f32::NAN, big: 123456789.0, yaml, extra: BTreeMap::from([("aaa".into(), 1), ("zeta".into(), 2)]),
        };
        let expected = serde_json::to_string(&serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(String::from_utf8(to_vec(&s).unwrap()).unwrap(), expected);
    }
}
