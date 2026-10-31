use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;

use quickcheck::quickcheck;
use rillfp::{
    AsyncLogger, AsyncReader, AsyncReaderResult, NonEmpty, SendReader, SendReaderResult,
    Validation, Writer,
};

struct ThreadWake(thread::Thread);

impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

// A small test-only executor that also supports futures waking from another thread.
fn block_on<T>(future: impl Future<Output = T>) -> T {
    let waker = Waker::from(Arc::new(ThreadWake(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => thread::park(),
        }
    }
}

fn assert_send_sync<T: Send + Sync>() {}

fn assert_send<T: Send>(_: &T) {}

#[test]
fn thread_safe_primitives_implement_send_and_sync() {
    assert_send_sync::<SendReader<'static, i32, i32>>();
    assert_send_sync::<SendReaderResult<'static, i32, i32, String>>();
    assert_send_sync::<AsyncReader<i32, i32>>();
    assert_send_sync::<AsyncReaderResult<i32, i32, String>>();
    assert_send_sync::<NonEmpty<i32>>();
    assert_send_sync::<Validation<i32, String>>();
    assert_send_sync::<Writer<i32, String>>();
}

#[test]
fn send_reader_can_borrow_captures_and_be_shared_across_scoped_threads() {
    let offset = 3;
    let reader = SendReader::new(|r: &i32| r + offset)
        .and_then(|value| SendReader::new(move |r: &i32| value + r));
    thread::scope(|scope| {
        let first = scope.spawn(|| reader.run(&2));
        let second = scope.spawn(|| reader.run(&4));
        assert_eq!(first.join().unwrap(), 7);
        assert_eq!(second.join().unwrap(), 11);
    });
}

#[test]
fn send_reader_result_composes_and_recovers_on_another_thread() {
    let reader = SendReaderResult::from_reader(SendReader::new(|r: &i32| *r))
        .and_then(|value| SendReaderResult::new(move |_: &i32| Err::<i32, _>(value)))
        .or_else(|error| SendReaderResult::new(move |r: &i32| Ok::<_, ()>(error + r)));

    assert_eq!(thread::spawn(move || reader.run(&3)).join().unwrap(), Ok(6));
}

quickcheck! {
    fn send_reader_matches_reader_arithmetic(environment: i16) -> bool {
        let reader = SendReader::new(|r: &i16| i64::from(*r))
            .map(|value| value + 1)
            .zip(SendReader::new(|r: &i16| i64::from(*r)));
        reader.run(&environment) == (i64::from(environment) + 1, i64::from(environment))
    }

    fn send_reader_result_maps_like_result(result: Result<i16, String>) -> bool {
        let reader = SendReaderResult::new(|_: &()| result.clone())
            .map(|value| i64::from(value) + 1);
        reader.run(&()) == result.clone().map(|value| i64::from(value) + 1)
    }

    fn async_reader_map_composes(environment: i16) -> bool {
        let reader = AsyncReader::new(|r: &i16| Box::pin(async move { i64::from(*r) }))
            .map(|value| value + 1)
            .map(|value| value * 2);
        block_on(reader.run(&environment)) == (i64::from(environment) + 1) * 2
    }

    fn async_reader_result_matches_result(result: Result<Option<i16>, String>) -> bool {
        let reader = AsyncReaderResult::new(|r: &Result<Option<i16>, String>| {
            Box::pin(async move { r.clone() })
        }).map(|value| value.map(|number| i64::from(number) + 1));
        block_on(reader.run(&result)) == result.map(|value| value.map(|number| i64::from(number) + 1))
    }
}

#[test]
fn async_reader_borrows_environment_across_suspension_and_returns_a_send_future() {
    let reader = AsyncReader::new(|environment: &String| {
        Box::pin(async move {
            let mut pending = true;
            std::future::poll_fn(move |context| {
                // Deliberately yield once and wake from another thread.
                if std::mem::take(&mut pending) {
                    let waker = context.waker().clone();
                    thread::spawn(move || waker.wake());
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
            .await;
            format!("{environment}: 21")
        })
    });
    let environment = "answer".to_owned();
    let future = reader.run(&environment);
    assert_send(&future);
    assert_eq!(block_on(future), "answer: 21");
}

#[test]
fn async_reader_is_lazy_reusable_and_supports_dependent_composition() {
    let calls = Arc::new(Mutex::new(0));
    let captured = Arc::clone(&calls);
    let reader = AsyncReader::new(move |r: &i32| {
        *captured.lock().unwrap() += 1;
        Box::pin(async move { *r })
    })
    .and_then(|first| AsyncReader::new(move |r: &i32| Box::pin(async move { first + r })));

    let unpolled = reader.run(&2);
    assert_eq!(*calls.lock().unwrap(), 0);
    drop(unpolled);
    assert_eq!(block_on(reader.run(&2)), 4);
    assert_eq!(block_on(reader.run(&3)), 6);
    assert_eq!(*calls.lock().unwrap(), 2);
}

#[test]
fn async_reader_supports_ask_pure_zip_local_and_sync_lifting() {
    let reader = AsyncReader::ask()
        .zip(AsyncReader::pure(3))
        .map(|(a, b)| a + b)
        .local(|text: &String| text.len());
    assert_eq!(block_on(reader.run(&"hello".to_owned())), 8);

    let lifted = AsyncReader::from_sync(SendReader::new(|r: &i32| r + 1));
    assert_eq!(block_on(lifted.run(&4)), 5);
}

#[test]
fn async_reader_result_short_circuits_bind_and_zip() {
    let reader = AsyncReaderResult::new(|_: &()| Box::pin(async { Err::<i32, _>("bad") }))
        .and_then(|_| -> AsyncReaderResult<(), i32, &str> { panic!("must not run") });
    let right = AsyncReaderResult::new(|_: &()| -> rillfp::BoxFuture<'_, Result<i32, &str>> {
        panic!("must not run")
    });
    assert_eq!(block_on(reader.zip(right).run(&())), Err("bad"));
}

#[test]
fn async_reader_result_supports_question_mark_recovery_and_same_environment() {
    let reader = AsyncReaderResult::new(|input: &Option<String>| {
        Box::pin(async move {
            let text = input.as_ref().ok_or("missing")?;
            text.parse::<i32>().map_err(|_| "invalid")
        })
    })
    .and_then(|number| {
        AsyncReaderResult::new(move |_: &Option<String>| Box::pin(async move { Ok(number + 1) }))
    });
    assert_eq!(block_on(reader.run(&Some("41".to_owned()))), Ok(42));
    assert_eq!(block_on(reader.run(&None)), Err("missing"));
    assert_eq!(
        block_on(reader.run(&Some("bad".to_owned()))),
        Err("invalid")
    );

    let recovered = reader.or_else(|error| {
        AsyncReaderResult::new(move |input: &Option<String>| {
            Box::pin(async move {
                Ok::<_, ()>((error.len() + input.as_ref().map_or(0, String::len)) as i32)
            })
        })
    });
    assert_eq!(block_on(recovered.run(&Some("bad".to_owned()))), Ok(10));
}

#[test]
fn async_reader_result_supports_lifting_mapping_local_and_unwrapping() {
    let reader = AsyncReaderResult::<usize, _, &str>::from_reader(AsyncReader::ask())
        .zip(AsyncReaderResult::from_reader(AsyncReader::pure(2)))
        .map(|(a, b)| a + b)
        .local(|text: &String| text.len());
    assert_eq!(block_on(reader.into_reader().run(&"abc".to_owned())), Ok(5));

    let lifted = AsyncReaderResult::from_sync(SendReaderResult::new(|_: &()| Err::<(), _>("bad")))
        .map_err(str::len);
    assert_eq!(block_on(lifted.run(&())), Err(3));
}

#[test]
fn async_readers_can_be_shared_between_threads() {
    let reader = Arc::new(AsyncReader::new(|r: &i32| Box::pin(async move { r + 1 })));
    let first = Arc::clone(&reader);
    let first = thread::spawn(move || block_on(first.run(&1)));
    let second = thread::spawn(move || block_on(reader.run(&2)));
    assert_eq!(first.join().unwrap(), 2);
    assert_eq!(second.join().unwrap(), 3);
}

#[test]
fn async_writer_emits_events_in_order_and_preserves_errors() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    let logger = move |event| {
        let events = Arc::clone(&captured);
        async move {
            events.lock().unwrap().push(event);
        }
    };
    let logger: &(dyn AsyncLogger<&str> + Sync) = &logger;
    let writer = Writer::new(Err::<(), _>("failed"), ["started", "finished"]);
    let future = writer.write_to_async(logger);
    assert_send(&future);
    assert!(events.lock().unwrap().is_empty());
    assert_eq!(block_on(future), Err("failed"));
    assert_eq!(*events.lock().unwrap(), ["started", "finished"]);
}

#[test]
fn send_readers_support_ask_pure_local_and_result_short_circuiting() {
    let reader = SendReader::ask()
        .zip(SendReader::pure(3))
        .map(|(a, b)| a + b)
        .local(|text: &String| text.len());
    assert_eq!(reader.run(&"hello".to_owned()), 8);

    let left = SendReaderResult::new(|_: &usize| Err::<i32, _>("bad"));
    let right = SendReaderResult::new(|_: &usize| -> Result<i32, &str> { panic!("must not run") });
    let reader = left
        .zip(right)
        .map_err(str::len)
        .local(|text: &String| text.len());
    assert_eq!(reader.into_reader().run(&"hello".to_owned()), Err(3));
}

#[test]
fn async_zip_waits_for_the_left_computation_before_starting_the_right() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let left_events = Arc::clone(&events);
    let left = AsyncReader::new(move |_: &()| {
        let events = Arc::clone(&left_events);
        Box::pin(async move {
            events.lock().unwrap().push("left started");
            let mut pending = true;
            std::future::poll_fn(move |context| {
                if std::mem::take(&mut pending) {
                    context.waker().wake_by_ref();
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
            .await;
            events.lock().unwrap().push("left finished");
            1
        })
    });
    let right_events = Arc::clone(&events);
    let right = AsyncReader::new(move |_: &()| {
        right_events.lock().unwrap().push("right started");
        Box::pin(async { 2 })
    });

    let reader = left.zip(right);
    let mut future = reader.run(&());
    let mut context = Context::from_waker(Waker::noop());
    assert_eq!(future.as_mut().poll(&mut context), Poll::Pending);
    assert_eq!(*events.lock().unwrap(), ["left started"]);
    assert_eq!(block_on(future), (1, 2));
    assert_eq!(
        *events.lock().unwrap(),
        ["left started", "left finished", "right started"]
    );
}

#[test]
fn cancelling_async_writer_does_not_submit_remaining_events() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    let logger = move |event: &'static str| {
        let events = Arc::clone(&captured);
        async move {
            events.lock().unwrap().push(event);
            std::future::pending::<()>().await;
        }
    };
    let writer = Writer::new(42, ["first", "second"]);
    let mut future = Box::pin(writer.write_to_async(&logger));
    let mut context = Context::from_waker(Waker::noop());

    assert_eq!(future.as_mut().poll(&mut context), Poll::Pending);
    assert_eq!(*events.lock().unwrap(), ["first"]);
    drop(future);
    assert_eq!(*events.lock().unwrap(), ["first"]);
}

#[test]
fn async_run_owns_its_factory_without_borrowing_the_reader() {
    let environment = "hello".to_owned();
    let future = {
        let reader = AsyncReader::new(|text: &String| Box::pin(async move { text.len() }));
        reader.run(&environment)
    };

    assert_eq!(block_on(future), 5);
}

#[test]
fn async_logger_implementations_can_borrow_themselves_across_await() {
    struct EventSink(Mutex<Vec<u32>>);

    impl AsyncLogger<u32> for EventSink {
        fn log(&self, event: u32) -> rillfp::BoxFuture<'_, ()> {
            Box::pin(async move {
                std::future::ready(()).await;
                self.0.lock().unwrap().push(event);
            })
        }
    }

    let logger = EventSink(Mutex::new(Vec::new()));
    assert_eq!(
        block_on(Writer::new(42, [1, 2]).write_to_async(&logger)),
        42
    );
    assert_eq!(*logger.0.lock().unwrap(), [1, 2]);
}
