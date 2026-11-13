# Validation accumulation investigation

Measured locally with rustc 1.100.0 on Linux, optimized Cargo bench profile,
20 samples, 0.2 s warmup, 0.5 s measurement, and 1,000 bootstrap resamples.
These are synthetic integer workloads, not application latency measurements.

## Findings

There are two independently observable costs:

- Repeated `NonEmpty::append(NonEmpty::new(error))` is much slower than `push`:
  approximately 11.7 us versus 1.6 us for 1,024 elements in an isolated run.
  Both use the same allocation count: a standalone Valgrind Memcheck probe
  reported 21 allocations including process/argument setup for either version.
  The two-byte difference in allocated bytes matched the different argument
  lengths (`append` versus `push`). This rules out allocating once per error
  in that probe; the iterator-chain path is the performance-sensitive difference.
- Simplifying append alone does not eliminate the validation gap. `map2`'s
  composition through `zip(...).map(...)` and compiler inlining decisions also
  matter. A direct match in a benchmark-local fold is substantially cheaper;
  moving that match into the library required an inline hint to obtain similar
  performance in the public-API benchmark.

## Experimental candidate (not applied)

The measured candidate combined these changes:

1. Implement `NonEmpty::append` as `self.push(other.head)` followed by
   `self.tail.extend(other.tail)`, returning `self`, with `#[inline]`.
2. Implement `Validation::map2` with a direct match over both validations,
   applying `f` only to two valid values, appending two error collections, and
   otherwise preserving the errors. Add `#[inline]` to this method too.

| 1,024 inputs | Original | Candidate | Direct Vec baseline in candidate run |
| --- | ---: | ---: | ---: |
| All valid | 0.461 us | 0.462 us | 0.235 us |
| All invalid | 14.9 us | 1.64 us | 1.08 us |
| Alternating invalid/valid | 11.0 us | 1.06 us | 0.785 us |

The error-heavy cases improved roughly 9–10x in this comparison. The success
path stayed approximately unchanged. Exact results were sensitive to code
layout/compiler decisions: even the unchanged direct success baseline varied
between about 0.23 and 0.46 us across builds. Neither direct matching nor an
inline hint alone was sufficient in the experiments; avoid claiming a universal
speedup from either in isolation.

The candidate passed the full rillfp test suite, including new QuickCheck
properties for append order and map2 equivalence to zip-then-map. Library source
was restored after the experiment; only diagnostic benchmarks, properties, and
these notes are retained. A follow-up optimization should remeasure the combined
candidate, include smaller cases and non-Copy error types, and check other
collection/composition workloads for regressions before adopting it.

## Reproduction

```sh
cargo bench -p rillfp --bench primitives -- 'validation/.*/.*/1024' \
  --warm-up-time 0.2 --measurement-time 0.5 --sample-size 20 --nresamples 1000 \
  --save-baseline investigation-before
cargo bench -p rillfp --bench primitives -- non_empty/accumulation \
  --warm-up-time 0.2 --measurement-time 0.5 --sample-size 20 --nresamples 1000
```

After applying a candidate, replace `--save-baseline investigation-before` with
`--baseline investigation-before` to compare. Local Criterion result files are
not committed. The `fused_map2` diagnostic removes the intermediate tuple but
still uses the library's append; `push` bypasses singleton append entirely.
