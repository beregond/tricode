//! Reed-Solomon error correction over GF(256), for codewords of up to 255 bytes.
//!
//! Codewords are systematic: the data bytes followed by `ecc` correction bytes. Byte `p` of an
//! `n`-byte codeword is the coefficient of `x^(n-1-p)`, and the generator polynomial has the roots
//! `α^0 … α^(ecc-1)`. A codeword survives `errors` wrong bytes at unknown positions plus `erasures`
//! bytes known to be unreliable as long as `2·errors + erasures <= ecc`.
//!
//! The decoder is written for hostile input: the image decoder hands it every misread grid,
//! flip and mask, so anything that doesn't decode must come back as `Err`, never as a panic.

/// The most bytes one codeword can hold.
pub const MAX_CODEWORD: usize = 255;

/// The field's primitive polynomial, x⁸ + x⁴ + x³ + x² + 1.
const PRIMITIVE: u16 = 0x11d;

struct Tables {
    /// `exp[i] = α^i`, doubled in length so a sum of two logarithms needs no reduction.
    exp: [u8; 512],
    /// `log[α^i] = i`; `log[0]` is unused.
    log: [u8; 256],
}

const TABLES: Tables = {
    let mut exp = [0u8; 512];
    let mut log = [0u8; 256];
    let mut x: u16 = 1;
    let mut i = 0;
    while i < 255 {
        exp[i] = x as u8;
        exp[i + 255] = x as u8;
        log[x as usize] = i as u8;
        x <<= 1;
        if x & 0x100 != 0 {
            x ^= PRIMITIVE;
        }
        i += 1;
    }
    exp[510] = exp[0];
    exp[511] = exp[1];
    Tables { exp, log }
};

fn mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    TABLES.exp[TABLES.log[a as usize] as usize + TABLES.log[b as usize] as usize]
}

/// `a / b`, or `None` for `b == 0`.
fn div(a: u8, b: u8) -> Option<u8> {
    if b == 0 {
        return None;
    }
    if a == 0 {
        return Some(0);
    }
    Some(TABLES.exp[TABLES.log[a as usize] as usize + 255 - TABLES.log[b as usize] as usize])
}

/// `α^power`, for any (also negative) power.
fn alpha_pow(power: isize) -> u8 {
    TABLES.exp[power.rem_euclid(255) as usize]
}

/// Evaluates a polynomial stored lowest degree first.
fn eval_low_first(poly: &[u8], x: u8) -> u8 {
    poly.iter().rev().fold(0, |acc, &c| mul(acc, x) ^ c)
}

/// Multiplies two polynomials stored lowest degree first.
fn poly_mul(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; a.len() + b.len() - 1];
    for (i, &ai) in a.iter().enumerate() {
        for (j, &bj) in b.iter().enumerate() {
            out[i + j] ^= mul(ai, bj);
        }
    }
    out
}

/// The generator polynomial `(x - α^0)…(x - α^(ecc-1))`, highest degree first, leading 1 included.
fn generator(ecc: usize) -> Vec<u8> {
    let mut g = vec![1u8];
    for j in 0..ecc {
        // Multiply by (x + α^j): highest degree first, so the new term shifts everything left.
        let root = alpha_pow(j as isize);
        let mut next = vec![0u8; g.len() + 1];
        for (i, &c) in g.iter().enumerate() {
            next[i] ^= c;
            next[i + 1] ^= mul(c, root);
        }
        g = next;
    }
    g
}

/// Appends `ecc` correction bytes to `data`. `data.len() + ecc` must not exceed [`MAX_CODEWORD`].
pub fn encode(data: &[u8], ecc: usize) -> Result<Vec<u8>, String> {
    if data.len() + ecc > MAX_CODEWORD {
        return Err(format!(
            "codeword of {} bytes exceeds {}",
            data.len() + ecc,
            MAX_CODEWORD
        ));
    }
    let g = generator(ecc);
    // Long division of data·x^ecc by g, keeping only the remainder.
    let mut remainder = vec![0u8; ecc];
    for &byte in data {
        let Some(&first) = remainder.first() else {
            break;
        };
        let factor = byte ^ first;
        remainder.rotate_left(1);
        if let Some(last) = remainder.last_mut() {
            *last = 0;
        }
        for (r, &gc) in remainder.iter_mut().zip(&g[1..]) {
            *r ^= mul(gc, factor);
        }
    }
    let mut codeword = data.to_vec();
    codeword.extend_from_slice(&remainder);
    Ok(codeword)
}

/// Syndromes `S_j = c(α^j)` for `j < ecc`.
fn syndromes(codeword: &[u8], ecc: usize) -> Vec<u8> {
    (0..ecc)
        .map(|j| {
            let x = alpha_pow(j as isize);
            codeword.iter().fold(0, |acc, &c| mul(acc, x) ^ c)
        })
        .collect()
}

/// Berlekamp-Massey: the shortest LFSR (connection polynomial, lowest degree first) that
/// generates `sequence`, and its length.
fn berlekamp_massey(sequence: &[u8]) -> (Vec<u8>, usize) {
    let mut c = vec![1u8];
    let mut b = vec![1u8];
    let mut length = 0usize;
    let mut shift = 1usize;
    let mut last_discrepancy = 1u8;
    for n in 0..sequence.len() {
        let mut d = sequence[n];
        for i in 1..=length.min(c.len() - 1) {
            d ^= mul(c[i], sequence[n - i]);
        }
        if d == 0 {
            shift += 1;
            continue;
        }
        // last_discrepancy is never 0: it starts at 1 and only takes non-zero discrepancies.
        let scale = div(d, last_discrepancy).unwrap_or(0);
        let previous = c.clone();
        if c.len() < b.len() + shift {
            c.resize(b.len() + shift, 0);
        }
        for (i, &bi) in b.iter().enumerate() {
            c[i + shift] ^= mul(scale, bi);
        }
        if 2 * length <= n {
            length = n + 1 - length;
            b = previous;
            last_discrepancy = d;
            shift = 1;
        } else {
            shift += 1;
        }
    }
    (c, length)
}

/// Corrects `codeword` in place, treating the bytes at `erasures` as unreliable, and returns how
/// many bytes were changed. Fails when there are more errors than `ecc` can correct; on failure
/// `codeword` is left as it was.
pub fn correct(codeword: &mut [u8], ecc: usize, erasures: &[usize]) -> Result<usize, String> {
    let n = codeword.len();
    if n > MAX_CODEWORD || ecc == 0 || ecc >= n {
        return Err(format!("invalid codeword: {} bytes, {} ecc", n, ecc));
    }
    let mut erasures: Vec<usize> = erasures.iter().copied().filter(|&p| p < n).collect();
    erasures.sort_unstable();
    erasures.dedup();
    if erasures.len() > ecc {
        return Err("too many erasures".to_string());
    }

    let synd = syndromes(codeword, ecc);
    if synd.iter().all(|&s| s == 0) {
        return Ok(0);
    }

    // Locator of byte p: X_p = α^(n-1-p).
    let locator = |p: usize| alpha_pow((n - 1 - p) as isize);

    // Erasure locator Γ(x) = Π (1 + X_p·x), lowest degree first.
    let gamma = erasures
        .iter()
        .fold(vec![1u8], |acc, &p| poly_mul(&acc, &[1, locator(p)]));

    // Forney syndromes: T(x) = S(x)·Γ(x) mod x^ecc. From index ν (the erasure count) on, they
    // only see the errors, so Berlekamp-Massey on them finds the error locator alone.
    let nu = erasures.len();
    let forney: Vec<u8> = poly_mul(&synd, &gamma)[..ecc].to_vec();
    let (sigma, errors) = berlekamp_massey(&forney[nu..]);
    if 2 * errors + nu > ecc {
        return Err("too many errors".to_string());
    }

    // Errata locator Λ = σ·Γ: its roots are X_p⁻¹ for every wrong or erased byte.
    let mut lambda = poly_mul(&sigma, &gamma);
    while lambda.len() > 1 && lambda.last() == Some(&0) {
        lambda.pop();
    }
    let degree = lambda.len() - 1;

    // Chien search: find every position whose locator is a root.
    let positions: Vec<usize> = (0..n)
        .filter(|&p| eval_low_first(&lambda, alpha_pow(-((n - 1 - p) as isize))) == 0)
        .collect();
    if positions.len() != degree {
        return Err("error locator doesn't match the codeword".to_string());
    }

    // Forney: error evaluator Ω(x) = S(x)·Λ(x) mod x^ecc, magnitude X·Ω(X⁻¹)/Λ'(X⁻¹).
    let omega: Vec<u8> = poly_mul(&synd, &lambda)[..ecc].to_vec();
    // Formal derivative in characteristic 2: only odd-degree terms survive.
    let lambda_prime: Vec<u8> = (1..lambda.len())
        .map(|i| if i % 2 == 1 { lambda[i] } else { 0 })
        .collect();

    let mut corrected = codeword.to_vec();
    for &p in &positions {
        let x = locator(p);
        let x_inv = alpha_pow(-((n - 1 - p) as isize));
        let denominator = eval_low_first(&lambda_prime, x_inv);
        let magnitude = div(mul(x, eval_low_first(&omega, x_inv)), denominator)
            .ok_or("error locator has a repeated root")?;
        corrected[p] ^= magnitude;
    }

    if syndromes(&corrected, ecc).iter().any(|&s| s != 0) {
        return Err("correction left the codeword inconsistent".to_string());
    }
    let changed = corrected
        .iter()
        .zip(codeword.iter())
        .filter(|(a, b)| a != b)
        .count();
    codeword.copy_from_slice(&corrected);
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{RngExt, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    #[test]
    fn field_tables_are_consistent() {
        for a in 1..=255u8 {
            assert_eq!(div(mul(a, 7), 7), Some(a));
            assert_eq!(mul(a, div(1, a).expect("non-zero")), 1);
        }
        assert_eq!(div(1, 0), None);
    }

    #[test]
    fn clean_codeword_has_zero_syndromes() {
        let codeword = encode(b"hello tricode", 10).expect("encode");
        assert_eq!(&codeword[..13], b"hello tricode");
        assert!(syndromes(&codeword, 10).iter().all(|&s| s == 0));
    }

    #[test]
    fn corrects_errors_and_erasures_up_to_capacity() {
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        for _ in 0..2000 {
            let n = rng.random_range(3..=MAX_CODEWORD);
            let ecc = rng.random_range(1..n);
            let data: Vec<u8> = (0..n - ecc).map(|_| rng.random()).collect();
            let clean = encode(&data, ecc).expect("encode");

            // Split the budget 2·errors + erasures <= ecc at random.
            let erasure_count = rng.random_range(0..=ecc);
            let error_count = rng.random_range(0..=(ecc - erasure_count) / 2);
            let mut positions: Vec<usize> = (0..n).collect();
            for i in 0..erasure_count + error_count {
                let j = rng.random_range(i..n);
                positions.swap(i, j);
            }
            let mut damaged = clean.clone();
            for &p in &positions[..erasure_count + error_count] {
                damaged[p] ^= rng.random_range(1..=255u8);
            }
            let erasures = &positions[..erasure_count];
            correct(&mut damaged, ecc, erasures).expect("within capacity");
            assert_eq!(damaged, clean, "n={n} ecc={ecc}");
        }
    }

    #[test]
    fn never_panics_on_garbage() {
        let mut rng = ChaCha8Rng::seed_from_u64(11);
        for _ in 0..20_000 {
            let n = rng.random_range(0..=MAX_CODEWORD + 2);
            let ecc = rng.random_range(0..=n + 1);
            let mut codeword: Vec<u8> = if rng.random_bool(0.5) || ecc >= n || n > MAX_CODEWORD {
                (0..n).map(|_| rng.random()).collect()
            } else {
                let data: Vec<u8> = (0..n - ecc).map(|_| rng.random()).collect();
                let mut c = encode(&data, ecc).expect("encode");
                for _ in 0..rng.random_range(0..=ecc) {
                    let p = rng.random_range(0..n);
                    c[p] = rng.random();
                }
                c
            };
            let erasures: Vec<usize> = (0..rng.random_range(0..=ecc + 2))
                .map(|_| rng.random_range(0..n + 2))
                .collect();
            let before = codeword.clone();
            if correct(&mut codeword, ecc, &erasures).is_err() {
                assert_eq!(codeword, before, "failed correction changed the codeword");
            }
        }
    }
}
