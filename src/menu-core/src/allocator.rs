//! Simple bump allocator for the texture data pool.
//!
//! Returns physical addresses within the reserved DDR3 region. Hands
//! out memory in strictly increasing address order with no deallocation
//! — adequate for v0 where textures are loaded once at startup and
//! live for the session. A more general allocator can replace this
//! without changing its public API.

/// Error type for [`BumpAllocator::alloc`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AllocError {
    /// Not enough space remaining for the requested allocation.
    #[error("out of memory: requested {requested} bytes, only {remaining} remain")]
    OutOfMemory { requested: u32, remaining: u32 },

    /// Alignment is not a power of two.
    #[error("alignment must be a power of two (got {0})")]
    BadAlignment(u32),
}

/// Bump allocator over a physical-address range.
#[derive(Debug, Clone)]
pub struct BumpAllocator {
    base: u32,
    end: u32,
    cursor: u32,
}

impl BumpAllocator {
    /// Create an allocator that hands out addresses in `[base, base + size)`.
    pub const fn new(base: u32, size: u32) -> Self {
        Self {
            base,
            end: base + size,
            cursor: base,
        }
    }

    /// Allocate `size` bytes with the given `align` (must be a power of
    /// two). Returns the physical address of the allocation.
    pub fn alloc(&mut self, size: u32, align: u32) -> Result<u32, AllocError> {
        if align == 0 || !align.is_power_of_two() {
            return Err(AllocError::BadAlignment(align));
        }
        let aligned = (self.cursor + align - 1) & !(align - 1);
        let next = aligned.checked_add(size).ok_or(AllocError::OutOfMemory {
            requested: size,
            remaining: self.remaining(),
        })?;
        if next > self.end {
            return Err(AllocError::OutOfMemory {
                requested: size,
                remaining: self.remaining(),
            });
        }
        self.cursor = next;
        Ok(aligned)
    }

    /// Reset the allocator back to empty. Invalidates all previously
    /// handed-out addresses.
    pub fn reset(&mut self) {
        self.cursor = self.base;
    }

    /// Bytes not yet allocated.
    #[inline]
    pub fn remaining(&self) -> u32 {
        self.end - self.cursor
    }

    /// Total capacity.
    #[inline]
    pub fn capacity(&self) -> u32 {
        self.end - self.base
    }

    /// Bytes currently allocated.
    #[inline]
    pub fn used(&self) -> u32 {
        self.cursor - self.base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hands_out_base_address_first() {
        let mut a = BumpAllocator::new(0x3200_0000, 1024);
        let p = a.alloc(16, 4).unwrap();
        assert_eq!(p, 0x3200_0000);
    }

    #[test]
    fn respects_alignment() {
        let mut a = BumpAllocator::new(0x3200_0000, 1024);
        let _ = a.alloc(5, 1).unwrap();
        // Cursor now at 0x3200_0005. Next alloc with align=16 must round up.
        let p = a.alloc(16, 16).unwrap();
        assert_eq!(p, 0x3200_0010);
    }

    #[test]
    fn rejects_non_power_of_two_alignment() {
        let mut a = BumpAllocator::new(0, 1024);
        assert_eq!(a.alloc(4, 3), Err(AllocError::BadAlignment(3)));
        assert_eq!(a.alloc(4, 0), Err(AllocError::BadAlignment(0)));
    }

    #[test]
    fn out_of_memory_when_exhausted() {
        let mut a = BumpAllocator::new(0, 32);
        a.alloc(16, 1).unwrap();
        a.alloc(12, 1).unwrap();
        let r = a.alloc(8, 1);
        assert!(matches!(r, Err(AllocError::OutOfMemory { .. })));
    }

    #[test]
    fn remaining_shrinks() {
        let mut a = BumpAllocator::new(0, 1024);
        assert_eq!(a.remaining(), 1024);
        a.alloc(100, 4).unwrap();
        assert_eq!(a.remaining(), 924);
        assert_eq!(a.used(), 100);
    }

    #[test]
    fn reset_restores_full_capacity() {
        let mut a = BumpAllocator::new(0x3000_0000, 1024);
        a.alloc(512, 1).unwrap();
        a.reset();
        assert_eq!(a.remaining(), 1024);
        assert_eq!(a.alloc(1, 1).unwrap(), 0x3000_0000);
    }
}
