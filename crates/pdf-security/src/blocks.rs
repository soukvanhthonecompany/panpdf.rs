pub(crate) struct Blocks<const WIDE: usize> {
    part: [u8; WIDE],
    filled: usize,
    length: u128,
}

impl<const WIDE: usize> Blocks<WIDE> {
    pub(crate) const fn new() -> Self {
        Self {
            part: [0; WIDE],
            filled: 0,
            length: 0,
        }
    }

    pub(crate) fn update(&mut self, mut data: &[u8], mut eat: impl FnMut(&[u8; WIDE])) {
        self.length += data.len() as u128;
        if self.filled > 0 {
            let wanted = (WIDE - self.filled).min(data.len());
            self.part[self.filled..self.filled + wanted].copy_from_slice(&data[..wanted]);
            self.filled += wanted;
            data = &data[wanted..];
            if self.filled < WIDE {
                return;
            }
            eat(&self.part);
            self.filled = 0;
        }
        let mut whole = data.chunks_exact(WIDE);
        for block in &mut whole {
            eat(block.try_into().expect("a whole block"));
        }
        let rest = whole.remainder();
        self.part[..rest.len()].copy_from_slice(rest);
        self.filled = rest.len();
    }

    pub(crate) fn finish(mut self, length_octets: usize, mut eat: impl FnMut(&[u8; WIDE])) {
        let bits = self.length * 8;
        let mut tail = Vec::with_capacity(WIDE * 2);
        tail.push(0x80u8);
        while !(self.filled + tail.len() + length_octets).is_multiple_of(WIDE) {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_be_bytes()[16 - length_octets..]);
        let was = self.length;
        self.update(&tail, &mut eat);
        self.length = was;
        debug_assert_eq!(self.filled, 0, "padding ends on a block boundary");
    }
}
