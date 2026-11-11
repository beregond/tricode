//! Error-correction framing: the payload wrapped as `[len u16 BE][payload][CRC-32 BE]`, split
//! into Reed-Solomon codewords and interleaved over the grid's bytes.

use crate::reed_solomon;

/// How much of every codeword is spent on error correction. Not stored in the code: the decoder
/// tries every level, and the frame's CRC tells which one fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EccLevel {
    /// About 7% of the bytes can be wrong.
    Low,
    /// About 15% of the bytes can be wrong.
    Medium,
    /// About 25% of the bytes can be wrong.
    Quartile,
    /// About 30% of the bytes can be wrong.
    High,
}

/// The level codes are generated at unless another is chosen.
pub const DEFAULT_ECC_LEVEL: EccLevel = EccLevel::Medium;

impl EccLevel {
    /// Every level, lowest first.
    pub const ALL: [EccLevel; 4] = [
        EccLevel::Low,
        EccLevel::Medium,
        EccLevel::Quartile,
        EccLevel::High,
    ];

    /// The level's name, as shown in the app.
    pub fn name(self) -> &'static str {
        match self {
            EccLevel::Low => "Low",
            EccLevel::Medium => "Medium",
            EccLevel::Quartile => "Quartile",
            EccLevel::High => "High",
        }
    }

    /// Largest share of the code's area an [`Overlay`](crate::Overlay) may take at this level. These are the
    /// solid covers `survives_a_covered_centre` checks, so a code with an overlay this large still
    /// decodes.
    pub fn max_overlay_share(self) -> f64 {
        match self {
            EccLevel::Low => 0.02,
            EccLevel::Medium => 0.06,
            EccLevel::Quartile => 0.15,
            EccLevel::High => 0.18,
        }
    }

    /// Share of every codeword's bytes that are correction bytes. A codeword corrects half as
    /// many wrong bytes as it has correction bytes (or as many erased ones).
    pub(crate) fn ecc_share(self) -> f64 {
        match self {
            EccLevel::Low => 0.14,
            EccLevel::Medium => 0.30,
            EccLevel::Quartile => 0.50,
            EccLevel::High => 0.60,
        }
    }
}

/// Bytes of a frame around the payload: a 2-byte length before it and a 4-byte CRC-32 after.
pub(crate) const FRAME_OVERHEAD: usize = 6;

/// Splits `capacity` bytes into Reed-Solomon codewords for `level`, as `(data, ecc)` byte counts.
/// Depends only on the grid's capacity and the level, so the decoder can rebuild it. Codewords
/// differ in length by at most one byte. Empty when the capacity is too small for any frame.
pub(crate) fn block_layout(capacity: usize, level: EccLevel) -> Vec<(usize, usize)> {
    if capacity < FRAME_OVERHEAD + 2 {
        return Vec::new();
    }
    let blocks = capacity.div_ceil(reed_solomon::MAX_CODEWORD);
    let (base, longer) = (capacity / blocks, capacity % blocks);
    (0..blocks)
        .map(|i| {
            let n = base + usize::from(i >= blocks - longer);
            // An even number of correction bytes, at least 2, leaving at least one data byte.
            let ecc = (2.0 * (n as f64 * level.ecc_share() / 2.0).round()) as usize;
            let ecc = ecc.clamp(2, n - 1);
            (n - ecc, ecc)
        })
        .collect()
}

/// Payload bytes a grid of `capacity` bytes holds at `level`.
pub(crate) fn payload_capacity(capacity: usize, level: EccLevel) -> usize {
    let data: usize = block_layout(capacity, level).iter().map(|&(k, _)| k).sum();
    data.saturating_sub(FRAME_OVERHEAD).min(u16::MAX as usize)
}

/// Where each byte of the grid comes from, as `(codeword, index)`: byte 0 of every codeword, then
/// byte 1 of every codeword, and so on. Interleaving spreads a damaged patch of the image over all
/// codewords instead of overwhelming one of them.
fn interleave_order(layout: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let longest = layout.iter().map(|&(k, e)| k + e).max().unwrap_or(0);
    (0..longest)
        .flat_map(|i| {
            layout
                .iter()
                .enumerate()
                .filter(move |&(_, &(k, e))| i < k + e)
                .map(move |(block, _)| (block, i))
        })
        .collect()
}

/// Encodes `payload` into exactly `capacity` grid bytes at `level`: the frame
/// `[length: u16 BE][payload][CRC-32 BE of length and payload]`, zero-padded to the data capacity,
/// split into Reed-Solomon codewords and interleaved.
pub(crate) fn encode_frame(
    payload: &[u8],
    capacity: usize,
    level: EccLevel,
) -> Result<Vec<u8>, String> {
    if payload.len() > payload_capacity(capacity, level) {
        return Err(format!(
            "{} payload bytes don't fit in {} grid bytes",
            payload.len(),
            capacity
        ));
    }
    let layout = block_layout(capacity, level);

    let mut frame = Vec::with_capacity(capacity);
    frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    frame.extend_from_slice(payload);
    frame.extend_from_slice(&crc32fast::hash(&frame).to_be_bytes());
    frame.resize(layout.iter().map(|&(k, _)| k).sum(), 0);

    let mut rest = frame.as_slice();
    let mut codewords = Vec::with_capacity(layout.len());
    for &(k, ecc) in &layout {
        let (data, tail) = rest.split_at(k);
        codewords.push(reed_solomon::encode(data, ecc)?);
        rest = tail;
    }
    Ok(interleave_order(&layout)
        .into_iter()
        .map(|(block, i)| codewords[block][i])
        .collect())
}

/// A frame recovered by [`decode_frame`].
pub(crate) struct DecodedFrame {
    pub(crate) payload: Vec<u8>,
    /// Bytes the error correction had to change.
    pub(crate) corrected_bytes: usize,
}

/// Reverse of [`encode_frame`] for one level. `erased[i]` marks grid bytes read from cells the
/// decoder couldn't tell apart, which cost half as much correction as wrong bytes. Fails unless
/// every codeword corrects and the CRC matches.
pub(crate) fn decode_frame(
    bytes: &[u8],
    erased: &[bool],
    level: EccLevel,
) -> Result<DecodedFrame, String> {
    let layout = block_layout(bytes.len(), level);
    if layout.is_empty() {
        return Err("grid too small".to_string());
    }
    let mut codewords: Vec<Vec<u8>> = layout
        .iter()
        .map(|&(k, e)| Vec::with_capacity(k + e))
        .collect();
    let mut erasures: Vec<Vec<usize>> = vec![Vec::new(); layout.len()];
    for (pos, (block, i)) in interleave_order(&layout).into_iter().enumerate() {
        codewords[block].push(bytes[pos]);
        if erased.get(pos).copied().unwrap_or(false) {
            erasures[block].push(i);
        }
    }

    let mut frame = Vec::new();
    let mut corrected_bytes = 0;
    for ((codeword, erasures), &(k, ecc)) in codewords.iter_mut().zip(&erasures).zip(&layout) {
        // Erasures can be wrong too (a cell that only looked unsure), so retry without them.
        let changed = reed_solomon::correct(codeword, ecc, erasures)
            .or_else(|e| {
                if erasures.is_empty() {
                    Err(e)
                } else {
                    reed_solomon::correct(codeword, ecc, &[])
                }
            })
            .map_err(|e| format!("{:?} codeword: {}", level, e))?;
        corrected_bytes += changed;
        frame.extend_from_slice(&codeword[..k]);
    }

    let &[hi, lo, ..] = frame.as_slice() else {
        return Err("frame too short".to_string());
    };
    let len = u16::from_be_bytes([hi, lo]) as usize;
    let end = 2 + len;
    let crc = frame
        .get(end..end + 4)
        .ok_or_else(|| format!("{:?}: length {} exceeds the frame", level, len))?;
    if crc32fast::hash(&frame[..end]).to_be_bytes() != crc {
        return Err(format!("{:?}: CRC mismatch", level));
    }
    Ok(DecodedFrame {
        payload: frame[2..end].to_vec(),
        corrected_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_log::test;

    #[test]
    fn block_layout_fills_the_capacity() {
        for capacity in [8, 9, 50, 254, 255, 256, 600, 1070, 5000] {
            for level in EccLevel::ALL {
                let layout = block_layout(capacity, level);
                let total: usize = layout.iter().map(|&(k, e)| k + e).sum();
                assert_eq!(total, capacity, "{capacity} {level:?}");
                for &(k, ecc) in &layout {
                    assert!(k >= 1 && ecc >= 2 && ecc % 2 == 0, "{capacity} {level:?}");
                    assert!(
                        k + ecc <= reed_solomon::MAX_CODEWORD,
                        "{capacity} {level:?}"
                    );
                }
            }
        }
        assert!(block_layout(7, EccLevel::Low).is_empty());
    }

    #[test]
    fn frame_round_trips_and_never_decodes_at_a_higher_level() {
        let payload = b"tricode frame";
        for capacity in [60, 300, 1070] {
            for level in EccLevel::ALL {
                let bytes = encode_frame(payload, capacity, level).expect("encode");
                assert_eq!(bytes.len(), capacity);
                let erased = vec![false; capacity];
                let frame = decode_frame(&bytes, &erased, level).expect("decode");
                assert_eq!(frame.payload, payload);
                // Lower levels may decode it too (see `try_unmix_decode`), higher ones must not.
                for higher in EccLevel::ALL.into_iter().filter(|&l| l as u8 > level as u8) {
                    assert!(
                        decode_frame(&bytes, &erased, higher).is_err(),
                        "{capacity}: {level:?} decoded as {higher:?}"
                    );
                }
            }
        }
        assert!(encode_frame(&[0; 100], 40, EccLevel::High).is_err());
    }
}
