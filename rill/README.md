# rillfp

Small, composable functional primitives for Rust. No runtime dependencies,
higher-kinded types, generic monad hierarchy, or monad transformers.

This is an initial, standard-library-based draft; the API is not
stable. It complements `Result` and `Option` rather than replacing them.

| Component | Purpose |
| --- | --- |
| `NonEmpty<T>` | A first element plus a tail, with non-emptiness preserved by construction |
| `Validation<T, E>` | Independent checks accumulating `NonEmpty<E>` errors in order |
| `Reader<'a, R, T>` | Lazy, reusable computations sharing an environment |
| `ReaderResult<'a, R, T, E>` | Reader with ordinary short-circuiting `Result` semantics |
| `SendReader<'a, R, T>` / `SendReaderResult<'a, R, T, E>` | Thread-safe synchronous readers |
| `AsyncReader<R, T>` / `AsyncReaderResult<R, T, E>` | Thread-safe, runtime-independent async readers |
| `Writer<T, Event>` | A value with ordered, deferred events |
| `Logger<Event>` | An explicit sink for application-defined structured events |
| `AsyncLogger<Event>` | An asynchronous sink with ordered Writer emission |

## Independent validation

```rust
use rillfp::Validation;

let name = Validation::from(None::<String>.ok_or("name is required"));
let age = Validation::<u8, _>::invalid("age must be positive");
let result = name.map2(age, |name, age| (name, age)).into_result();

assert_eq!(
    result.unwrap_err().into_vec(),
    ["name is required", "age must be positive"],
);
```

`zip` and `map2` accumulate errors left-to-right. `and_then` handles dependent
checks and therefore short-circuits. `From<Result<T, E>>` lifts a single error;
`into_result` returns `Result<T, NonEmpty<E>>` without discarding errors.

`NonEmpty::from_vec` returns `Option<NonEmpty<T>>`; `new` and `from_head_tail`
construct non-empty collections directly. Borrowed, mutable, and owned iteration
are supported. No operation can remove the last element.

## Shared environments

```rust
use rillfp::{Reader, ReaderResult};

struct Config { prefix: String, enabled: bool }
let config = Config { prefix: "hello".into(), enabled: true };

let greeting = Reader::new(|c: &Config| c.prefix.clone())
    .map(|prefix| format!("{prefix}, world"));
assert_eq!(greeting.run(&config), "hello, world");

let checked = ReaderResult::new(|c: &Config| {
    c.enabled.then_some(()).ok_or("disabled")
}).and_then(|()| ReaderResult::from_reader(
    Reader::new(|c: &Config| c.prefix.clone())
));
assert_eq!(checked.run(&config), Ok("hello".to_owned()));
```

Readers run only on `run(&environment)`, can run repeatedly, and may borrow
closure captures. `map`, `and_then`, and `zip` compose computations; `local`
adapts an outer environment into an owned inner environment. `Reader::ask`
clones the environment; use `Reader::new` for a smaller projection.

The explicit `'a` is the closure-capture lifetime, not a promise that returned
values borrow from the runtime environment. Readers use boxed `Fn` closures
and dynamic dispatch, without `Send`/`Sync` requirements. They do not enforce
purity. `ReaderResult::zip` runs left-to-right
and does not run the right-hand computation after a left-hand error.

## Thread-safe readers

Use `SendReader` and `SendReaderResult` when computations need to move between
threads or be shared by reference. They provide the same operations as their
local counterparts, but their closures must be `Send + Sync`. Borrowed captures
are supported with scoped threads; ordinary `thread::spawn` requires owned or
`'static` captures. `pure` additionally requires a `Send + Sync` value because
it captures that value. Environment and output types otherwise need only the
bounds required by the actual work you perform.

```rust
use rillfp::SendReader;

let offset = 2;
let reader = SendReader::new(|value: &i32| value + offset);
std::thread::scope(|scope| {
    let task = scope.spawn(|| reader.run(&40));
    assert_eq!(task.join().unwrap(), 42);
});
```

`NonEmpty`, `Validation`, and `Writer` already implement `Send` and `Sync`
automatically when their contents do. A synchronous logger can be shared as
`dyn Logger<Event> + Send + Sync` when its implementation supports those traits.

## Asynchronous computations

`AsyncReader` and `AsyncReaderResult` own `Send + Sync + 'static` future factories
and return `BoxFuture<'r, T>` (`Pin<Box<dyn Future<Output = T> + Send + 'r>>`).
Use `Box::pin(async move { ... })` in the constructor. The future can borrow
`&R` across `.await`, but the output cannot borrow from that environment.
Environments must be `Sync + 'static`; results and errors must be `Send + 'static`.
Here, `'static` means those types contain no non-static borrows; the environment
value itself may be a local variable. Use `Arc` to share captured state.

```rust
use rillfp::AsyncReaderResult;

async fn increment(input: &Option<String>) -> Result<u32, &'static str> {
    let reader = AsyncReaderResult::new(|input: &Option<String>| Box::pin(async move {
        let text = input.as_ref().ok_or("missing number")?;
        text.parse::<u32>().map_err(|_| "invalid number")
    })).and_then(|number| AsyncReaderResult::new(move |_: &Option<String>| {
        Box::pin(async move { number.checked_add(1).ok_or("overflow") })
    }));
    reader.run(input).await
}
# let _ = increment;
```

The library does not provide an executor or spawn tasks. Await these futures in
your existing runtime. `run` defers even factory invocation until polling;
cloning a reader shares its factory, and each run creates an independent future.
`zip` awaits left-to-right, **not concurrently**; the fallible version does not
start the right side after a left-side error. `local` keeps an owned inner
environment alive across `.await` and additionally requires that inner type to
be `Send`. `from_sync` lifts a `SendReader` or `SendReaderResult` with `'static`
captures, but does not offload blocking work. Dropping a future cancels that run
without rolling back completed effects.

## Deferred structured events

```rust
use std::cell::RefCell;
use rillfp::Writer;

#[derive(Debug, PartialEq)]
enum Event { Created { id: u64 } }

let events = RefCell::new(Vec::new());
let logger = |event| events.borrow_mut().push(event);
let computation = Writer::pure(42)
    .and_then(|id| Writer::new(id, [Event::Created { id }]));

assert!(events.borrow().is_empty());
assert_eq!(computation.write_to(&logger), 42);
assert_eq!(*events.borrow(), [Event::Created { id: 42 }]);
```

Writer composition is eager, but logging is deferred. `map` preserves events;
`and_then` and `zip` append events left-to-right. `into_parts` extracts the value
and events without emitting them. `write_to` consumes a writer and emits events
in order, without transactional guarantees if a logger panics.

`Logger<Event>` accepts your own structs or enums; closures implementing
`Fn(Event)` automatically implement it. Custom adapters can implement the trait,
including as `dyn Logger<Event>`. Backends control filtering, delivery, and
failure policy; there is no global logger or prescribed event schema.

`AsyncLogger<Event>` is the asynchronous equivalent. It supports trait objects
and methods whose futures borrow the logger. Closures returning `Send + 'static`
futures implement it automatically:

```rust
use std::sync::{Arc, Mutex};
use rillfp::Writer;

async fn record() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    let logger = move |event: u32| {
        let events = Arc::clone(&captured);
        async move { events.lock().unwrap().push(event); }
    };
    let value = Writer::new(42, [1, 2]).write_to_async(&logger).await;
    assert_eq!(value, 42);
    assert_eq!(*events.lock().unwrap(), [1, 2]);
}
# let _ = record;
```

`write_to_async` requires a `Sync` logger and `Send` values/events. It returns a
`Send` future, awaiting each event before submitting the next. Cancellation or
panic can leave a partially emitted sequence; there is no transactional delivery
guarantee. Both logging traits leave backend failures to the implementation.

## Development

QuickCheck is the only direct dev dependency. Property tests cover invariants,
composition laws, error order, and interoperability; example tests cover
short-circuiting, laziness, explicit logging, thread sharing, async suspension,
and cancellation. Async tests use a small, standard-library-only test executor.

```sh
cargo test
QUICKCHECK_TESTS=1000 cargo test --test primitives
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo doc --no-deps
cargo run --example workflow
```

## Package name

The crate is named `rillfp`: a rill is a small flowing stream, fitting a library
focused on composing computations. The name was unregistered on crates.io when
checked on November 13, 2026; recheck before publication. Publishing is disabled
in `Cargo.toml` pending a license decision. For local use, depend on this
directory with `rillfp = { path = "/path/to/rill" }`.
