//! Vector normalisation and dot products.

/// Scale a vector to unit length. Returns `None` for empty, zero or non-finite vectors.
pub fn normalize(vector: &[f32]) -> Option<Vec<f32>> {
    if vector.is_empty() || vector.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm == 0.0 || !norm.is_finite() {
        return None;
    }
    Some(vector.iter().map(|v| v / norm).collect())
}

/// Number of independent partial sums in [`dot`]. Float addition is not
/// associative, so a single running sum is a serial chain the compiler cannot
/// vectorize; separate accumulators let it use SIMD lanes.
const DOT_LANES: usize = 16;

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    let (a_chunks, a_rest) = a[..len].as_chunks::<DOT_LANES>();
    let (b_chunks, b_rest) = b[..len].as_chunks::<DOT_LANES>();
    let mut acc = [0.0f32; DOT_LANES];
    for (x, y) in a_chunks.iter().zip(b_chunks) {
        for i in 0..DOT_LANES {
            acc[i] += x[i] * y[i];
        }
    }
    let rest: f32 = a_rest.iter().zip(b_rest).map(|(x, y)| x * y).sum();
    acc.iter().sum::<f32>() + rest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_produces_unit_vectors() {
        let v = normalize(&[3.0, 4.0]).unwrap();
        assert!((v[0] - 0.6).abs() < 1e-6);
        assert!((v[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn normalize_rejects_degenerate_vectors() {
        assert_eq!(normalize(&[]), None);
        assert_eq!(normalize(&[0.0, 0.0]), None);
        assert_eq!(normalize(&[f32::NAN, 1.0]), None);
        assert_eq!(normalize(&[f32::INFINITY, 1.0]), None);
    }

    fn naive_dot(a: &[f32], b: &[f32]) -> f64 {
        a.iter()
            .zip(b)
            .map(|(x, y)| f64::from(*x) * f64::from(*y))
            .sum()
    }

    fn pseudo_random_vector(len: usize, seed: u32) -> Vec<f32> {
        let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn dot_matches_naive_sum_around_chunk_boundaries() {
        for len in 0..=70 {
            let a = pseudo_random_vector(len, 1);
            let b = pseudo_random_vector(len, 2);
            let expected = naive_dot(&a, &b);
            let actual = f64::from(dot(&a, &b));
            assert!(
                (actual - expected).abs() < 1e-4,
                "len={len}: {actual} != {expected}"
            );
        }
    }

    #[test]
    fn dot_ignores_extra_elements_of_the_longer_vector() {
        let a = pseudo_random_vector(40, 3);
        let b = pseudo_random_vector(23, 4);
        assert!((f64::from(dot(&a, &b)) - naive_dot(&a[..23], &b)).abs() < 1e-4);
        assert!((f64::from(dot(&b, &a)) - naive_dot(&b, &a[..23])).abs() < 1e-4);
    }

    #[test]
    fn dot_of_unit_vector_with_itself_is_one() {
        let v = normalize(&pseudo_random_vector(768, 5)).unwrap();
        assert!((dot(&v, &v) - 1.0).abs() < 1e-5);
    }

    /// Compare `dot` with a plain sequential sum on a corpus-sized workload:
    /// `cargo test -p rss-centr-core --release bench_dot -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_dot() {
        use std::hint::black_box;
        use std::time::Instant;

        fn sequential(a: &[f32], b: &[f32]) -> f32 {
            a.iter().zip(b).map(|(x, y)| x * y).sum()
        }

        const DIM: usize = 768;
        const ITEMS: usize = 50_000;
        const ROUNDS: u32 = 10;
        let query = pseudo_random_vector(DIM, 0);
        let index: Vec<Vec<f32>> = (1..=ITEMS as u32)
            .map(|seed| pseudo_random_vector(DIM, seed))
            .collect();

        let time = |f: fn(&[f32], &[f32]) -> f32| {
            let start = Instant::now();
            let mut total = 0.0;
            for _ in 0..ROUNDS {
                for v in &index {
                    total += f(black_box(&query), black_box(v));
                }
            }
            black_box(total);
            start.elapsed() / ROUNDS
        };

        let sequential_time = time(sequential);
        let dot_time = time(dot);
        println!(
            "{ITEMS} x {DIM}: sequential {sequential_time:?}, dot {dot_time:?} ({:.1}x)",
            sequential_time.as_secs_f64() / dot_time.as_secs_f64()
        );
    }

    quickcheck::quickcheck! {
        fn prop_normalize_yields_unit_length(v: Vec<i16>) -> bool {
            let v: Vec<f32> = v.into_iter().map(f32::from).collect();
            match normalize(&v) {
                Some(n) => (n.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-3,
                None => v.iter().all(|x| *x == 0.0),
            }
        }

        fn prop_dot_matches_naive_sum(pairs: Vec<(i8, i8)>) -> bool {
            let a: Vec<f32> = pairs.iter().map(|(x, _)| f32::from(*x) / 128.0).collect();
            let b: Vec<f32> = pairs.iter().map(|(_, y)| f32::from(*y) / 128.0).collect();
            (f64::from(dot(&a, &b)) - naive_dot(&a, &b)).abs() < 1e-3
        }
    }
}
