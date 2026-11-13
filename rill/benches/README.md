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

## Initial experimental candidate (superseded)

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

The initial candidate passed the full rillfp test suite, including new QuickCheck
properties for append order and map2 equivalence to zip-then-map. Library source
was restored after the experiment; only diagnostic benchmarks, properties, and
these notes are retained. A follow-up optimization should remeasure the combined
candidate, include smaller cases and non-Copy error types, and check other
collection/composition workloads for regressions before adopting it.

## Owned-error follow-up (applied)

Expanded the suite to 1, 2, 8, 32, and 1,024 inputs and added owned `String`
errors. String benchmarks consume owned input vectors on both paths; batched
setup clones synthetic fixtures outside timing. They isolate composition and
moving errors rather than string formatting/cloning. Batched result destruction
is also outside timing, unlike the borrowed integer benchmarks.

The original tuple-match candidate regressed larger all-invalid String cases
(about 15.2 us to 18.6 us for 1,024 checks). It was **not** adopted unchanged.
The revised implementation uses:

- A nested match on `self` and then `other` in `map2`, with `#[inline]`, avoiding
  both the intermediate zipped validation and a tuple of the two inputs.
- A reservation for the entire incoming NonEmpty before pushing its head and
  extending its tail, with `#[inline]`. Reserving first avoids growing twice
  when appending two multi-element collections.

Representative before/after estimates from the final comparison:

| Workload | Original | Revised |
| --- | ---: | ---: |
| Integer, 2 failures | 51.8 ns | 25.9 ns |
| Integer, 8 failures | 168 ns | 66.8 ns |
| Integer, 1,024 failures | 15.0 us | 1.76 us |
| Integer, 1,024 mixed checks | 11.1 us | 1.08 us |
| String, 2 failures | 59.7 ns | 37.7 ns |
| String, 8 failures | 180 ns | 88.5 ns |
| String, 32 failures | 583 ns | 262 ns |
| String, 1,024 failures | 15.2 us | 5.52 us |
| String, 1,024 mixed checks | 14.7 us | 3.27 us |
| String-typed input, 1,024 successes | 5.31 us | 1.52 us |

All validation sizes/scenarios improved in that comparison, but the revised
implementation is still slower than direct Vec accumulation in larger cases.
Baseline: 20 samples, 0.1 s warmup, 0.3 s measurement; final comparison:
20 samples, 0.2 s warmup, 0.5 s measurement. Both used 1,000 resamples.

Also compared Writer accumulation, NonEmpty mapping, and bulk append with owned
strings. The unreserved prototype regressed bulk appends; reserving removed the
double-growth issue. Collection timings were variable, including unchanged Vec
baselines. A focused repeat put 1,024-element mapping near the original (172 ns
versus 166 ns), and 1,024-per-side owned append at 1.78 us versus 1.92 us.
Not every case improved: two-element owned append measured about 42 ns versus
39 ns, Writer's 1,024-event case about 1.64 us versus 1.43 us, and 32-per-side
owned append remained noisy at roughly 130 ns versus 100 ns. These short runs
do not establish regression-free performance or explain every code-layout effect.

The revised library implementation is retained. The API and ordered error
accumulation semantics are unchanged. QuickCheck additionally verifies combining
multi-element owned error collections without invoking the success combiner.

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
