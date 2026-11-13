use std::future::Future;
use std::hint::black_box;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use rillfp::{AsyncReader, NonEmpty, Reader, Validation, Writer};

const CHAIN_LENGTHS: [usize; 3] = [1, 8, 32];
const COLLECTION_SIZES: [usize; 5] = [1, 2, 8, 32, 1024];

fn reader_chain(length: usize) -> Reader<'static, u64, u64> {
    (0..length).fold(Reader::new(|value: &u64| *value), |reader, _| {
        reader.map(|value| value.wrapping_add(1))
    })
}

fn readers(c: &mut Criterion) {
    let mut construction = c.benchmark_group("reader/construction");
    for length in CHAIN_LENGTHS {
        construction.bench_with_input(
            BenchmarkId::from_parameter(length),
            &length,
            |b, &length| {
                b.iter(|| black_box(reader_chain(black_box(length))));
            },
        );
    }
    construction.finish();

    let mut execution = c.benchmark_group("reader/execution");
    for length in CHAIN_LENGTHS {
        let reader = reader_chain(length);
        assert_eq!(reader.run(&42), 42 + length as u64);
        execution.bench_with_input(BenchmarkId::new("rill", length), &length, |b, _| {
            b.iter(|| reader.run(black_box(&42)));
        });
        execution.bench_with_input(BenchmarkId::new("direct", length), &length, |b, &length| {
            b.iter(|| {
                (0..black_box(length)).fold(black_box(42_u64), |value, _| value.wrapping_add(1))
            });
        });
    }
    execution.finish();
}

// These CPU-only futures must complete on the first poll. This deliberately
// measures composition rather than executor scheduling, wakeups, or I/O.
fn poll_ready<F: Future>(future: F) -> F::Output {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("benchmark future unexpectedly suspended"),
    }
}

fn async_reader_chain(length: usize) -> AsyncReader<u64, u64> {
    (0..length).fold(
        AsyncReader::new(|value: &u64| Box::pin(async move { *value })),
        |reader, _| reader.map(|value| value.wrapping_add(1)),
    )
}

fn async_readers(c: &mut Criterion) {
    let mut construction = c.benchmark_group("async_reader/construction");
    for length in CHAIN_LENGTHS {
        construction.bench_with_input(
            BenchmarkId::from_parameter(length),
            &length,
            |b, &length| {
                b.iter(|| black_box(async_reader_chain(black_box(length))));
            },
        );
    }
    construction.finish();

    let mut execution = c.benchmark_group("async_reader/execution");
    for length in CHAIN_LENGTHS {
        let reader = async_reader_chain(length);
        assert_eq!(poll_ready(reader.run(&42)), 42 + length as u64);
        execution.bench_with_input(BenchmarkId::new("rill", length), &length, |b, _| {
            b.iter(|| poll_ready(reader.run(black_box(&42))));
        });
        execution.bench_with_input(BenchmarkId::new("direct", length), &length, |b, &length| {
            b.iter(|| {
                poll_ready(async {
                    let mut value = black_box(42_u64);
                    for _ in 0..black_box(length) {
                        value = async { value.wrapping_add(1) }.await;
                    }
                    value
                })
            });
        });
    }
    execution.finish();
}

fn validate(values: &[Result<u64, usize>]) -> Result<u64, Vec<usize>> {
    values
        .iter()
        .copied()
        .fold(Validation::Valid(0), |sum, value| {
            sum.map2(value.into(), |sum, value| sum + value)
        })
        .into_result()
        .map_err(NonEmpty::into_vec)
}

fn validate_direct(values: &[Result<u64, usize>]) -> Result<u64, Vec<usize>> {
    let mut sum = 0;
    let mut errors = Vec::new();
    for value in values {
        match value {
            Ok(value) => sum += value,
            Err(error) => errors.push(*error),
        }
    }
    if errors.is_empty() {
        Ok(sum)
    } else {
        Err(errors)
    }
}

// Diagnostic equivalents: separate map2's intermediate tuple from appending
// singleton errors. Keep these beside the public-API workload for comparison.
fn validate_fused(values: &[Result<u64, usize>]) -> Result<u64, Vec<usize>> {
    values
        .iter()
        .copied()
        .fold(Validation::Valid(0), |sum, value| {
            match (sum, Validation::from(value)) {
                (Validation::Valid(sum), Validation::Valid(value)) => {
                    Validation::Valid(sum + value)
                }
                (Validation::Invalid(left), Validation::Invalid(right)) => {
                    Validation::Invalid(left.append(right))
                }
                (Validation::Invalid(errors), _) | (_, Validation::Invalid(errors)) => {
                    Validation::Invalid(errors)
                }
            }
        })
        .into_result()
        .map_err(NonEmpty::into_vec)
}

fn validate_push(values: &[Result<u64, usize>]) -> Result<u64, Vec<usize>> {
    values
        .iter()
        .copied()
        .fold(Validation::Valid(0), |sum, value| match (sum, value) {
            (Validation::Valid(sum), Ok(value)) => Validation::Valid(sum + value),
            (Validation::Invalid(mut errors), Err(error)) => {
                errors.push(error);
                Validation::Invalid(errors)
            }
            (Validation::Invalid(errors), Ok(_)) => Validation::Invalid(errors),
            (Validation::Valid(_), Err(error)) => Validation::invalid(error),
        })
        .into_result()
        .map_err(NonEmpty::into_vec)
}

fn validation(c: &mut Criterion) {
    for scenario in ["valid", "invalid", "mixed"] {
        let mut group = c.benchmark_group(format!("validation/{scenario}"));
        for size in COLLECTION_SIZES {
            let values: Vec<_> = (0..size)
                .map(|index| match scenario {
                    "invalid" => Err(index),
                    "mixed" if index % 2 == 0 => Err(index),
                    _ => Ok(index as u64),
                })
                .collect();
            assert_eq!(validate(&values), validate_direct(&values));
            assert_eq!(validate_fused(&values), validate_direct(&values));
            assert_eq!(validate_push(&values), validate_direct(&values));
            group.bench_with_input(BenchmarkId::new("rill", size), &values, |b, values| {
                b.iter(|| validate(black_box(values)));
            });
            group.bench_with_input(BenchmarkId::new("direct", size), &values, |b, values| {
                b.iter(|| validate_direct(black_box(values)));
            });
            group.bench_with_input(
                BenchmarkId::new("fused_map2", size),
                &values,
                |b, values| {
                    b.iter(|| validate_fused(black_box(values)));
                },
            );
            group.bench_with_input(BenchmarkId::new("push", size), &values, |b, values| {
                b.iter(|| validate_push(black_box(values)));
            });
        }
        group.finish();
    }
}

fn validate_owned(values: Vec<Result<u64, String>>) -> Result<u64, Vec<String>> {
    values
        .into_iter()
        .fold(Validation::Valid(0), |sum, value| {
            sum.map2(value.into(), |sum, value| sum + value)
        })
        .into_result()
        .map_err(NonEmpty::into_vec)
}

fn validate_owned_direct(values: Vec<Result<u64, String>>) -> Result<u64, Vec<String>> {
    let mut sum = 0;
    let mut errors = Vec::new();
    for value in values {
        match value {
            Ok(value) => sum += value,
            Err(error) => errors.push(error),
        }
    }
    if errors.is_empty() {
        Ok(sum)
    } else {
        Err(errors)
    }
}

fn owned_validation(c: &mut Criterion) {
    for scenario in ["valid", "invalid", "mixed"] {
        let mut group = c.benchmark_group(format!("validation_owned/{scenario}"));
        for size in COLLECTION_SIZES {
            let values: Vec<_> = (0..size)
                .map(|index| match scenario {
                    "invalid" => Err(format!("field {index}: value is required")),
                    "mixed" if index % 2 == 0 => Err(format!("field {index}: value is required")),
                    _ => Ok(index as u64),
                })
                .collect();
            assert_eq!(
                validate_owned(values.clone()),
                validate_owned_direct(values.clone())
            );
            // Clone fixtures outside timing so we measure moving owned errors,
            // not formatting or cloning their strings. Both paths consume inputs.
            group.bench_with_input(BenchmarkId::new("rill", size), &values, |b, values| {
                b.iter_batched(
                    || values.clone(),
                    |values| validate_owned(black_box(values)),
                    BatchSize::SmallInput,
                );
            });
            group.bench_with_input(BenchmarkId::new("direct", size), &values, |b, values| {
                b.iter_batched(
                    || values.clone(),
                    |values| validate_owned_direct(black_box(values)),
                    BatchSize::SmallInput,
                );
            });
        }
        group.finish();
    }
}

fn accumulate_events(size: usize) -> (usize, Vec<usize>) {
    (0..size)
        .fold(Writer::pure(0), |writer, event| {
            writer.and_then(|count| Writer::new(count + 1, [event]))
        })
        .into_parts()
}

fn accumulate_events_direct(size: usize) -> (usize, Vec<usize>) {
    let mut events = Vec::new();
    let mut count = 0;
    for event in 0..size {
        count += 1;
        events.push(event);
    }
    (count, events)
}

fn writers(c: &mut Criterion) {
    let mut group = c.benchmark_group("writer/accumulation");
    for size in COLLECTION_SIZES {
        assert_eq!(accumulate_events(size), accumulate_events_direct(size));
        group.bench_with_input(BenchmarkId::new("rill", size), &size, |b, &size| {
            b.iter(|| accumulate_events(black_box(size)));
        });
        group.bench_with_input(BenchmarkId::new("direct", size), &size, |b, &size| {
            b.iter(|| accumulate_events_direct(black_box(size)));
        });
    }
    group.finish();
}

fn non_empty(c: &mut Criterion) {
    let mut group = c.benchmark_group("non_empty/map");
    for size in COLLECTION_SIZES {
        let values = NonEmpty::from_head_tail(0_u64, 1..size as u64);
        let vector: Vec<_> = (0..size as u64).collect();
        assert_eq!(
            values.clone().map(|value| value + 1).into_vec(),
            vector.iter().map(|value| value + 1).collect::<Vec<_>>()
        );
        group.bench_with_input(BenchmarkId::new("rill", size), &values, |b, values| {
            b.iter_batched(
                || values.clone(),
                |values| black_box(values).map(|value| value + 1),
                BatchSize::SmallInput,
            );
        });
        group.bench_with_input(BenchmarkId::new("vec", size), &vector, |b, values| {
            b.iter_batched(
                || values.clone(),
                |values| {
                    black_box(values)
                        .into_iter()
                        .map(|value| value + 1)
                        .collect::<Vec<_>>()
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn owned_append(c: &mut Criterion) {
    let mut group = c.benchmark_group("non_empty/append_owned");
    for size in COLLECTION_SIZES {
        let left =
            NonEmpty::from_head_tail("left 0".to_owned(), (1..size).map(|i| format!("left {i}")));
        let right = NonEmpty::from_head_tail(
            "right 0".to_owned(),
            (1..size).map(|i| format!("right {i}")),
        );
        let left_vec: Vec<_> = left.iter().cloned().collect();
        let right_vec: Vec<_> = right.iter().cloned().collect();
        assert_eq!(
            left.clone().append(right.clone()).into_vec(),
            left.iter().chain(right.iter()).cloned().collect::<Vec<_>>()
        );
        group.bench_function(BenchmarkId::new("rill", size), |b| {
            b.iter_batched(
                || (left.clone(), right.clone()),
                |(left, right)| black_box(left).append(black_box(right)),
                BatchSize::SmallInput,
            );
        });
        group.bench_function(BenchmarkId::new("vec", size), |b| {
            b.iter_batched(
                || (left_vec.clone(), right_vec.clone()),
                |(mut left, mut right)| {
                    black_box(&mut left).append(black_box(&mut right));
                    left
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn error_accumulation(c: &mut Criterion) {
    let mut group = c.benchmark_group("non_empty/accumulation");
    for size in COLLECTION_SIZES {
        group.bench_with_input(
            BenchmarkId::new("append_singleton", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    (1..black_box(size)).fold(NonEmpty::new(0), |errors, error| {
                        errors.append(NonEmpty::new(error))
                    })
                });
            },
        );
        group.bench_with_input(BenchmarkId::new("push", size), &size, |b, &size| {
            b.iter(|| {
                (1..black_box(size)).fold(NonEmpty::new(0), |mut errors, error| {
                    errors.push(error);
                    errors
                })
            });
        });
        group.bench_with_input(BenchmarkId::new("extend_once", size), &size, |b, &size| {
            b.iter(|| {
                (1..black_box(size)).fold(NonEmpty::new(0), |mut errors, error| {
                    errors.extend(std::iter::once(error));
                    errors
                })
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    readers,
    async_readers,
    validation,
    owned_validation,
    writers,
    non_empty,
    owned_append,
    error_accumulation
);
criterion_main!(benches);
