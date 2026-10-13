//! The message stream that drives a settle: the same byte payloads the game's
//! managed side sends to the native simulation, tagged with the frame (tick)
//! before which they are delivered.
//!
//! On-disk format (`messages.bin`, little endian):
//! `b"SIMMSG01"`, width i32, height i32, frame payload length u32 + bytes,
//! message count u32, then per message: frame u16, id i32, length u32, bytes.

pub const ELEMENTS: i32 = 1108437482;
pub const DISEASES: i32 = 825301935;
pub const INITIALIZE_CELLS: i32 = 2062421945;
pub const START: i32 = -931446686;
pub const FRAME: i32 = -775326397;
pub const PREPARE: i32 = 1078620451;
pub const MODIFY_CELL: i32 = -1252920804;
pub const BACKWALL: i32 = -605421515;

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub frame: u16,
    pub id: i32,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stream {
    pub width: i32,
    pub height: i32,
    /// Payload of the per-tick frame message (NewGameFrame).
    pub frame: Vec<u8>,
    pub messages: Vec<Message>,
}

const MAGIC: &[u8; 8] = b"SIMMSG01";

impl Stream {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend(self.width.to_le_bytes());
        out.extend(self.height.to_le_bytes());
        out.extend((self.frame.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.frame);
        out.extend((self.messages.len() as u32).to_le_bytes());
        for m in &self.messages {
            out.extend(m.frame.to_le_bytes());
            out.extend(m.id.to_le_bytes());
            out.extend((m.payload.len() as u32).to_le_bytes());
            out.extend_from_slice(&m.payload);
        }
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut r = crate::io::Reader::new(bytes);
        if r.take(8)? != MAGIC {
            return Err("not a message stream".into());
        }
        let width = r.i32()?;
        let height = r.i32()?;
        let n = r.u32()? as usize;
        let frame = r.take(n)?.to_vec();
        let count = r.u32()? as usize;
        let mut messages = Vec::with_capacity(count);
        for _ in 0..count {
            let frame = r.u16()?;
            let id = r.i32()?;
            let n = r.u32()? as usize;
            messages.push(Message { frame, id, payload: r.take(n)?.to_vec() });
        }
        if !r.is_empty() {
            return Err("trailing bytes in message stream".into());
        }
        Ok(Stream { width, height, frame, messages })
    }

    pub fn find(&self, id: i32) -> Option<&Message> {
        self.messages.iter().find(|m| m.id == id)
    }
}
