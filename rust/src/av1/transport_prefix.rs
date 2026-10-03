//! Bounded AV1 ownership-prefix reader for the paired CBS slice transport.
//! Reads authoritative refresh/order syntax, not compressed tile contents or
//! the remaining frame header. Unsupported syntax fails closed.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    Truncated,
    Invalid,
    Unsupported,
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}
impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn read(&mut self, n: u8) -> Result<u32, Error> {
        if n > 32 {
            return Err(Error::Invalid);
        }
        let end = self.pos.checked_add(usize::from(n)).ok_or(Error::Invalid)?;
        if end > self.data.len().checked_mul(8).ok_or(Error::Invalid)? {
            return Err(Error::Truncated);
        }
        let mut value = 0;
        while self.pos < end {
            value = (value << 1) | u32::from((self.data[self.pos / 8] >> (7 - self.pos % 8)) & 1);
            self.pos += 1;
        }
        Ok(value)
    }
    fn flag(&mut self) -> Result<bool, Error> {
        Ok(self.read(1)? != 0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sequence {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub order_bits: u8,
    pub screen_tools: u8,
    pub integer_mv: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Frame {
    pub frame_type: u8,
    pub order_hint: u8,
    pub refresh: u8,
    pub primary_ref: u8,
    pub error_resilient: bool,
    pub prefix_bits: usize,
}

pub(crate) struct Obu<'a> {
    pub kind: u8,
    pub body: &'a [u8],
    pub body_offset: usize,
    pub end: usize,
}

pub(crate) fn obus(data: &[u8]) -> Result<Vec<Obu<'_>>, Error> {
    if data.is_empty() || data.len() > 64 * 1024 * 1024 {
        return Err(Error::Invalid);
    }
    let mut offset = 0;
    let mut result = Vec::new();
    while offset < data.len() {
        if result.len() == 3 {
            return Err(Error::Unsupported);
        }
        let header = data[offset];
        offset += 1;
        if header & 0x81 != 0 || header & 2 == 0 {
            return Err(Error::Invalid);
        }
        if header & 4 != 0 {
            // Layered transport has not been qualified. Only layer0 accepted.
            if *data.get(offset).ok_or(Error::Truncated)? != 0 {
                return Err(Error::Unsupported);
            }
            offset += 1;
        }
        let mut size = 0u64;
        let mut complete = false;
        for shift in (0..56).step_by(7) {
            let byte = *data.get(offset).ok_or(Error::Truncated)?;
            offset += 1;
            size |= u64::from(byte & 127) << shift;
            if byte & 128 == 0 {
                complete = true;
                break;
            }
        }
        if !complete {
            return Err(Error::Invalid);
        }
        let end = offset
            .checked_add(usize::try_from(size).map_err(|_| Error::Invalid)?)
            .filter(|end| *end <= data.len())
            .ok_or(Error::Truncated)?;
        result.push(Obu {
            kind: (header >> 3) & 15,
            body: &data[offset..end],
            body_offset: offset,
            end,
        });
        offset = end;
    }
    Ok(result)
}

pub(crate) fn sequence(data: &[u8]) -> Result<Sequence, Error> {
    let mut b = Bits::new(data);
    if b.read(3)? != 0 || b.flag()? || b.flag()? {
        return Err(Error::Unsupported);
    }
    // Timing/model syntax is deliberately rejected, rather than skipped.
    if b.flag()? {
        return Err(Error::Unsupported);
    }
    let initial_delay = b.flag()?;
    if b.read(5)? != 0 || b.read(12)? != 0 {
        return Err(Error::Unsupported);
    }
    if b.read(5)? > 7 {
        b.read(1)?;
    }
    if initial_delay && b.flag()? {
        b.read(4)?;
    }
    let width_bits = b.read(4)? as u8 + 1;
    let height_bits = b.read(4)? as u8 + 1;
    let width = b.read(width_bits)? + 1;
    let height = b.read(height_bits)? + 1;
    if b.flag()? {
        return Err(Error::Unsupported);
    } // frame IDs
    b.read(3)?; // superblock/filter/intra-edge flags
    b.read(4)?; // interintra/masked/warped/dual-filter flags
    let order = b.flag()?;
    if order {
        b.read(2)?;
    }
    let screen_tools = if b.flag()? { 2 } else { b.read(1)? as u8 };
    let integer_mv = if screen_tools != 0 {
        if b.flag()? { 2 } else { b.read(1)? as u8 }
    } else {
        2
    };
    let order_bits = if order { b.read(3)? as u8 + 1 } else { 0 };
    b.read(3)?; // superres/cdef/restoration
    let bit_depth = if b.flag()? { 10 } else { 8 };
    if b.flag()? {
        return Err(Error::Unsupported);
    } // monochrome
    if b.flag()? {
        let primaries = b.read(8)?;
        let transfer = b.read(8)?;
        let matrix = b.read(8)?;
        if primaries == 1 && transfer == 13 && matrix == 0 {
            return Err(Error::Unsupported);
        }
    }
    b.read(1)?; // range
    if b.read(2)? == 3 {
        return Err(Error::Invalid);
    } // chroma sample position
    b.read(1)?; // separate UV deltas
    if b.flag()? {
        return Err(Error::Unsupported);
    } // film grain
    if !b.flag()? {
        return Err(Error::Invalid);
    } // trailing one
    while !b.pos.is_multiple_of(8) {
        if b.flag()? {
            return Err(Error::Invalid);
        }
    }
    if b.pos != data.len() * 8 {
        return Err(Error::Invalid);
    }
    Ok(Sequence {
        width,
        height,
        bit_depth,
        order_bits,
        screen_tools,
        integer_mv,
    })
}

pub(crate) fn frame(data: &[u8], seq: &Sequence) -> Result<Frame, Error> {
    let mut b = Bits::new(data);
    if b.flag()? {
        return Err(Error::Unsupported);
    } // show-existing is client aliasing
    let frame_type = b.read(2)? as u8;
    if frame_type > 2 || !b.flag()? {
        return Err(Error::Unsupported);
    }
    let error_resilient = frame_type == 0 || b.flag()?;
    b.read(1)?; // disable_cdf_update
    let screen = if seq.screen_tools == 2 {
        b.flag()?
    } else {
        seq.screen_tools != 0
    };
    if screen && seq.integer_mv == 2 {
        b.read(1)?;
    }
    b.read(1)?; // frame_size_override
    let order_hint = b.read(seq.order_bits)? as u8;
    let primary_ref = if frame_type != 1 || error_resilient {
        7
    } else {
        b.read(3)? as u8
    };
    let refresh = if frame_type == 0 {
        255
    } else {
        b.read(8)? as u8
    };
    Ok(Frame {
        frame_type,
        order_hint,
        refresh,
        primary_ref,
        error_resilient,
        prefix_bits: b.pos,
    })
}
