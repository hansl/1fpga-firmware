//! SPSC command ring writer (PROTOCOL.md §4).
//!
//! The ring buffer lives in DDR3. The host is the single producer
//! (writes commands, advances `tail`); the FPGA is the single consumer
//! (reads commands, advances `head`). This module encapsulates the
//! host-side bookkeeping — it does not perform memory barriers or
//! `RING_TAIL` register writes itself; the caller drives those after
//! a batch of pushes to amortize the bridge transaction cost.

use crate::protocol::Command;
use crate::protocol::commands::EncodeError;

/// Error type for [`RingWriter::push`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RingError {
    /// Not enough free space to accommodate the command, accounting for
    /// any NOP-pad required to wrap around the end of the ring.
    #[error("not enough free space in ring (need {need} bytes, free {free})")]
    Full { need: usize, free: usize },

    /// Command encoding failed. Only possible if the internal encoder
    /// produced a different length than it reported, which would be a
    /// bug.
    #[error("command encoding failed: {0}")]
    Encode(#[from] EncodeError),

    /// The ring size is not a power of two.
    #[error("ring size must be a power of two (got {0} bytes)")]
    InvalidSize(usize),
}

/// Writes commands into a ring-buffer slice, handling NOP-pad wrap at
/// the end of the ring.
///
/// The writer maintains the host-side `tail` and a cached last-known
/// `head`. The caller is responsible for:
///
/// 1. Calling [`RingWriter::observe_head`] periodically so the writer
///    knows how much space is free (otherwise pushes fail with `Full`
///    once the ring appears full).
/// 2. Issuing a memory barrier after a batch of pushes and writing
///    the new [`RingWriter::tail`] to the FPGA's `RING_TAIL` register.
/// 3. Optionally poking `RING_KICK`.
pub struct RingWriter<'a> {
    buffer: &'a mut [u8],
    size: usize,
    mask: usize,
    tail: usize,
    head: usize,
}

impl<'a> RingWriter<'a> {
    /// Construct a ring writer over the given byte slice. The length
    /// must be a power of two (see `RING_SIZE` in `crate::mem`).
    pub fn new(buffer: &'a mut [u8]) -> Result<Self, RingError> {
        Self::with_state(buffer, 0, 0)
    }

    /// Like [`new`](Self::new), but seed the host-tracked tail and the
    /// cached head — useful when re-attaching the writer to a buffer
    /// between command batches without losing the rolling tail position.
    pub fn with_state(buffer: &'a mut [u8], tail: u32, head: u32) -> Result<Self, RingError> {
        let size = buffer.len();
        if !size.is_power_of_two() {
            return Err(RingError::InvalidSize(size));
        }
        Ok(Self {
            buffer,
            size,
            mask: size - 1,
            tail: (tail as usize) & (size - 1),
            head: (head as usize) & (size - 1),
        })
    }

    /// Current byte offset of the host-side tail. The caller writes
    /// this value into the FPGA's `RING_TAIL` register to hand work
    /// over to the consumer.
    #[inline]
    pub fn tail(&self) -> u32 {
        self.tail as u32
    }

    /// Update the cached head pointer. Call this after polling the
    /// FPGA's `RING_HEAD` register; `RingWriter` uses this value to
    /// determine free space.
    #[inline]
    pub fn observe_head(&mut self, head: u32) {
        self.head = (head as usize) & self.mask;
    }

    /// Bytes currently free in the ring. Per §4.2 we leave one word
    /// of margin so "empty" and "full" are unambiguous.
    #[inline]
    pub fn free_bytes(&self) -> usize {
        // free = (head - tail - 4) mod size
        (self.head.wrapping_sub(self.tail).wrapping_sub(4)) & self.mask
    }

    /// Bytes remaining before the ring's physical end (useful for
    /// deciding whether a NOP-pad is needed).
    #[inline]
    fn contiguous_bytes_to_end(&self) -> usize {
        self.size - self.tail
    }

    /// Append `cmd` to the ring, inserting a NOP-pad if the command
    /// wouldn't fit contiguously before the end of the ring.
    pub fn push(&mut self, cmd: &Command) -> Result<(), RingError> {
        let need = cmd.encoded_len();
        debug_assert_eq!(need % 4, 0, "encoded length must be 4-byte aligned");

        // Determine whether we need a NOP-pad to wrap to offset 0.
        let to_end = self.contiguous_bytes_to_end();
        let pad_bytes = if need > to_end { to_end } else { 0 };
        let total_need = pad_bytes + need;

        let free = self.free_bytes();
        if total_need > free {
            return Err(RingError::Full {
                need: total_need,
                free,
            });
        }

        // Write NOP-pad (if any) contiguously from tail to ring end.
        if pad_bytes > 0 {
            debug_assert_eq!(pad_bytes % 4, 0);
            // NOP with `padding_words = (pad_bytes / 4) - 1`. The NOP
            // header itself consumes the first 4 bytes.
            let nop_words = (pad_bytes / 4) - 1;
            // `padding_words` is 8 bits; this fits because the largest
            // possible pad is ring_size - 4, which at 1 MB is far more
            // than 255 * 4 bytes. For very large rings we must split
            // into multiple NOPs.
            if nop_words <= u8::MAX as usize {
                let pad_cmd = Command::Nop {
                    padding_words: nop_words as u8,
                };
                pad_cmd.encode(&mut self.buffer[self.tail..self.tail + pad_bytes])?;
            } else {
                // Split into N NOPs of 256 words each (1024 bytes).
                // For our 1 MB ring (with ≤ 1020 bytes pad max) this
                // branch is unreachable, but handle it correctly for
                // larger rings.
                let mut written = 0;
                while written < pad_bytes {
                    let remaining = pad_bytes - written;
                    let chunk_bytes = core::cmp::min(remaining, (u8::MAX as usize + 1) * 4);
                    let chunk_words = (chunk_bytes / 4) - 1;
                    let chunk = Command::Nop {
                        padding_words: chunk_words as u8,
                    };
                    chunk.encode(
                        &mut self.buffer[self.tail + written..self.tail + written + chunk_bytes],
                    )?;
                    written += chunk_bytes;
                }
            }
            self.tail = (self.tail + pad_bytes) & self.mask;
            debug_assert_eq!(self.tail, 0, "NOP-pad should have wrapped tail to 0");
        }

        // Write the actual command.
        cmd.encode(&mut self.buffer[self.tail..self.tail + need])?;
        self.tail = (self.tail + need) & self.mask;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{BlendMode, Command, Filter, Rect, Rgba};

    fn make_ring(size: usize) -> Vec<u8> {
        vec![0u8; size]
    }

    #[test]
    fn new_rejects_non_power_of_two() {
        let mut buf = make_ring(100);
        assert!(matches!(
            RingWriter::new(&mut buf),
            Err(RingError::InvalidSize(100))
        ));
    }

    #[test]
    fn push_writes_command_at_tail() {
        let mut buf = make_ring(64);
        let mut w = RingWriter::new(&mut buf).unwrap();
        w.observe_head(0);
        // head=0, tail=0, free = (0 - 0 - 4) mod 64 = 60 bytes
        assert_eq!(w.free_bytes(), 60);

        w.push(&Command::Present).unwrap();
        // Present is 4 bytes, tail advances to 4
        assert_eq!(w.tail(), 4);

        // First four bytes are the Present header LE
        assert_eq!(&buf[0..4], &[0x00, 0x00, 0x00, 0x01]);
    }

    #[test]
    fn push_wraps_around_with_nop_pad() {
        let mut buf = make_ring(64);
        let mut w = RingWriter::new(&mut buf).unwrap();
        w.observe_head(0);

        // Fill the ring with 12 Present commands (48 bytes) so tail
        // lands at 48, leaving 16 bytes contiguous to end.
        for _ in 0..12 {
            w.push(&Command::Present).unwrap();
        }
        assert_eq!(w.tail(), 48);

        // A FillRect is 4 + 3*4 = 16 bytes; fits contiguously (16 to end).
        let fill = Command::FillRect {
            dst: Rect::new(0, 0, 1, 1),
            color: Rgba::WHITE,
            blend: BlendMode::Opaque,
            ignore_clip: false,
        };
        assert_eq!(fill.encoded_len(), 16);

        // Pretend FPGA has consumed everything so head is at tail → free is size-4.
        w.observe_head(48);
        w.push(&fill).unwrap();
        // tail wraps to 0 after 16-byte write at position 48
        assert_eq!(w.tail(), 0);
        // No NOP-pad was needed since 16 fits exactly.
        // Verify the FillRect was written at offset 48, not 0.
        assert_eq!(&buf[48..52], &[0x00, 0x00, 0x03, 0x10]);
    }

    #[test]
    fn push_inserts_nop_pad_when_command_crosses_end() {
        // 64-byte ring, write things to land tail at 52 (12 bytes to end).
        // Then push FillRect (16 bytes). Since 16 > 12, a NOP-pad of 12
        // bytes is emitted (3 words of padding: header + 2 zero words,
        // so NOP header has length_w=2), then FillRect at offset 0.
        let mut buf = make_ring(64);
        let mut w = RingWriter::new(&mut buf).unwrap();
        w.observe_head(0);

        // 13 Present commands = 52 bytes (head must stay ≥ that for space).
        // But head=0 and we need free ≥ 52+12+16 = 80, and the ring is only
        // 64 bytes; impossible. So use a bigger ring.
        let mut buf = make_ring(128);
        let mut w = RingWriter::new(&mut buf).unwrap();
        w.observe_head(0);

        for _ in 0..29 {
            w.push(&Command::Present).unwrap(); // 29 × 4 = 116 bytes
        }
        assert_eq!(w.tail(), 116);
        // 128 - 116 = 12 bytes contiguous to end.

        // Now pretend FPGA has drained enough to make room.
        w.observe_head(100);
        // free = (100 - 116 - 4) mod 128 = (100 - 120) mod 128 = 108 - ugh let me recompute:
        // (100 - 116 - 4) is -20, and mod 128 is 108. Plenty of room.

        let fill = Command::FillRect {
            dst: Rect::new(0, 0, 1, 1),
            color: Rgba::WHITE,
            blend: BlendMode::Opaque,
            ignore_clip: false,
        };
        w.push(&fill).unwrap();
        // tail wraps: 116 + 12 (pad) = 128 = 0, then +16 (fill) = 16.
        assert_eq!(w.tail(), 16);

        // NOP-pad at offset 116: header opcode=0x00, length_w=2, flags=0
        // → LE bytes [00, 00, 02, 00]
        assert_eq!(&buf[116..120], &[0x00, 0x00, 0x02, 0x00]);
        // Padding words: zeroed
        assert_eq!(&buf[120..128], &[0; 8]);
        // FillRect at offset 0
        assert_eq!(&buf[0..4], &[0x00, 0x00, 0x03, 0x10]);
    }

    #[test]
    fn push_returns_full_when_ring_is_full() {
        let mut buf = make_ring(16);
        let mut w = RingWriter::new(&mut buf).unwrap();
        w.observe_head(0);
        // Ring has 16 - 4 = 12 bytes free.
        w.push(&Command::Present).unwrap(); // 4
        w.push(&Command::Present).unwrap(); // 8
        w.push(&Command::Present).unwrap(); // 12 → tail at 12, free now 0
        let r = w.push(&Command::Present);
        assert!(matches!(r, Err(RingError::Full { .. })));
    }

    #[test]
    fn copy_rect_with_tint_encodes_correctly() {
        let mut buf = make_ring(128);
        let mut w = RingWriter::new(&mut buf).unwrap();
        w.observe_head(0);
        let cmd = Command::CopyRect {
            tex_id: 0x01,
            src: Rect::new(0, 0, 8, 8),
            dst: Rect::new(10, 20, 8, 8),
            blend: BlendMode::SrcAlpha,
            filter: Filter::Nearest,
            tint: Some(Rgba::new(0xFF, 0x80, 0x40, 0xFF)),
        };
        assert_eq!(cmd.encoded_len(), 4 + 6 * 4);
        w.push(&cmd).unwrap();
        assert_eq!(w.tail(), 28);
        // Header: flags = blend(1) | tint_en(0x10) = 0x11
        assert_eq!(&buf[0..4], &[0x11, 0x00, 0x06, 0x11]);
    }
}
