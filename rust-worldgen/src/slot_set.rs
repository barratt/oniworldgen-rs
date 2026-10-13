//! Enumeration behavior of the game's Mono HashSet, for small identity sets.
//! Enumerators walk live slots, not hash buckets. Removed slots are reused LIFO.
#[derive(Default)]
pub(crate) struct SlotSet {
    slots: Vec<Option<usize>>,
    free: Vec<usize>,
}
impl SlotSet {
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ { self.slots.iter().flatten().copied() }
    pub fn contains(&self, value: usize) -> bool { self.slots.contains(&Some(value)) }
    pub fn clear(&mut self) { self.slots.clear(); self.free.clear(); }
    pub fn insert(&mut self, value: usize) {
        if self.contains(value) { return; }
        if let Some(index) = self.free.pop() { self.slots[index] = Some(value); }
        else { self.slots.push(Some(value)); }
    }
    pub fn remove(&mut self, value: usize) -> bool {
        let Some(index) = self.slots.iter().position(|v| *v == Some(value)) else { return false; };
        self.slots[index] = None;
        self.free.push(index);
        if self.free.len() == self.slots.len() { self.clear(); }
        true
    }
    pub fn retain(&mut self, keep: impl Fn(usize) -> bool) {
        let remove: Vec<_> = self.iter().filter(|v| !keep(*v)).collect();
        for value in remove { self.remove(value); }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enumeration_reuses_removed_slots_instead_of_appending() {
        let mut s = SlotSet::default();
        for n in [4, 7, 2, 9] { s.insert(n); }
        s.remove(7); s.remove(2); s.insert(8); s.insert(6);
        assert_eq!(s.iter().collect::<Vec<_>>(), [4, 6, 8, 9]);
        s.retain(|_| false); s.insert(11); s.insert(5); s.insert(11);
        assert_eq!(s.iter().collect::<Vec<_>>(), [11, 5]);
    }
}
