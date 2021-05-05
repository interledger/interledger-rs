use std::fmt;

/// Slice as hex string debug formatter, doesn't require allocating a string.
#[derive(PartialEq, Eq)]
pub struct HexString<'a>(pub &'a [u8]);

impl<'a> fmt::Debug for HexString<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(fmt, "{:02x}", b)?;
        }
        Ok(())
    }
}
