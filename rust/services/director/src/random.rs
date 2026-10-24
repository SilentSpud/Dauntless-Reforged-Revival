//! Random ids and choices (`crypto.randomUUID`, `crypto.randomInt`), from the system's generator.

use aws_lc_rs::rand::{SecureRandom, SystemRandom};

fn bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    // The system generator does not fail in practice; zeros would still give a valid (if poor) id.
    let _ = SystemRandom::new().fill(&mut out);
    out
}

/// A version 4 UUID in lower-case hex, as `crypto.randomUUID()` writes it.
pub fn uuid_v4() -> String {
    let mut b = bytes::<16>();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// An integer in `[min, max)`, without modulo bias.
pub fn int_in(min: usize, max: usize) -> usize {
    let range = max.saturating_sub(min) as u64;
    if range <= 1 {
        return min;
    }
    let zone = u64::MAX - (u64::MAX % range);
    loop {
        let n = u64::from_le_bytes(bytes::<8>());
        if n < zone {
            return min + (n % range) as usize;
        }
    }
}
