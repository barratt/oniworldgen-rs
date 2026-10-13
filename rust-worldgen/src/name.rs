//! Interned element and disease names for grid cells. A world has a few
//! hundred thousand cells but only dozens of distinct names, so cells hold a
//! copyable reference to one shared copy of each name instead of their own
//! `String`. A `Name` dereferences to `str`, compares and orders as its text
//! and serializes as a string.
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Default)]
pub struct Name(&'static str);

impl Name {
    pub fn new(name: &str) -> Name {
        // Each thread keeps the names it has seen, so that threads generating
        // in parallel rarely meet at the shared table.
        thread_local! { static SEEN: std::cell::RefCell<crate::fxhash::FxHashSet<&'static str>> = Default::default(); }
        SEEN.with(|seen| {
            if let Some(&interned) = seen.borrow().get(name) { return Name(interned); }
            let interned = Name::intern(name);
            seen.borrow_mut().insert(interned);
            Name(interned)
        })
    }

    fn intern(name: &str) -> &'static str {
        static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
        let mut names = NAMES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&interned) = names.get(name) { return interned; }
        let interned: &'static str = Box::leak(name.to_owned().into_boxed_str());
        names.insert(interned);
        interned
    }

    pub fn as_str(&self) -> &'static str { self.0 }
}

impl std::ops::Deref for Name {
    type Target = str;
    fn deref(&self) -> &str { self.0 }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str { self.0 }
}

impl std::borrow::Borrow<str> for Name {
    fn borrow(&self) -> &str { self.0 }
}

impl PartialEq for Name {
    fn eq(&self, other: &Name) -> bool { std::ptr::eq(self.0, other.0) || self.0 == other.0 }
}
impl Eq for Name {}
impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool { self.0 == other }
}
impl PartialEq<&str> for Name {
    fn eq(&self, other: &&str) -> bool { self.0 == *other }
}
impl PartialEq<String> for Name {
    fn eq(&self, other: &String) -> bool { self.0 == other.as_str() }
}
impl PartialEq<Name> for str {
    fn eq(&self, other: &Name) -> bool { self == other.0 }
}
impl PartialEq<Name> for &str {
    fn eq(&self, other: &Name) -> bool { *self == other.0 }
}
impl PartialEq<Name> for String {
    fn eq(&self, other: &Name) -> bool { self.as_str() == other.0 }
}
impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Name) -> Option<std::cmp::Ordering> { Some(self.cmp(other)) }
}
impl Ord for Name {
    fn cmp(&self, other: &Name) -> std::cmp::Ordering { self.0.cmp(other.0) }
}
impl std::hash::Hash for Name {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) { self.0.hash(state) }
}

impl std::fmt::Debug for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { std::fmt::Debug::fmt(self.0, f) }
}
impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.0) }
}

impl From<&str> for Name {
    fn from(name: &str) -> Name { Name::new(name) }
}
impl From<&String> for Name {
    fn from(name: &String) -> Name { Name::new(name) }
}
impl From<String> for Name {
    fn from(name: String) -> Name { Name::new(&name) }
}
impl From<Name> for String {
    fn from(name: Name) -> String { name.0.to_owned() }
}

impl serde::Serialize for Name {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> { serializer.serialize_str(self.0) }
}
impl<'de> serde::Deserialize<'de> for Name {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Name, D::Error> {
        let name = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Ok(Name::new(&name))
    }
}
