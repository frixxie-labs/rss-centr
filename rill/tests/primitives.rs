use std::cell::{Cell, RefCell};

use quickcheck::quickcheck;
use rillfp::{Logger, NonEmpty, Reader, ReaderResult, Validation, Writer};

quickcheck! {
    fn validation_map2_matches_zip_then_map(left: Result<i16, String>, right: Result<i16, String>) -> bool {
        let left = Validation::from(left);
        let right = Validation::from(right);
        let combine = |a: i16, b: i16| i32::from(a) + i32::from(b);
        left.clone().map2(right.clone(), combine)
            == left.zip(right).map(|(a, b)| combine(a, b))
    }

    fn non_empty_append_matches_vec_concatenation(
        left_head: i32, left_tail: Vec<i32>, right_head: i32, right_tail: Vec<i32>
    ) -> bool {
        let left = NonEmpty::from_head_tail(left_head, left_tail);
        let right = NonEmpty::from_head_tail(right_head, right_tail);
        let expected: Vec<_> = left.iter().chain(right.iter()).copied().collect();
        left.append(right).into_vec() == expected
    }

    fn non_empty_round_trip(values: Vec<i32>) -> bool {
        match NonEmpty::from_vec(values.clone()) {
            Some(non_empty) => !values.is_empty() && non_empty.into_vec() == values,
            None => values.is_empty(),
        }
    }

    fn non_empty_map_preserves_length(head: i16, tail: Vec<i16>) -> bool {
        let values = NonEmpty::from_head_tail(head, tail);
        let length = values.len();
        values.map(i64::from).len() == length
    }

    fn non_empty_map_composes(head: i16, tail: Vec<i16>) -> bool {
        let values = NonEmpty::from_head_tail(head, tail);
        values.clone().map(i64::from).map(|x| x + 1)
            == values.map(|x| i64::from(x) + 1)
    }

    fn validation_result_round_trip(result: Result<i32, String>) -> bool {
        let validation = Validation::from(result.clone());
        validation.into_result().map_err(|errors| errors.into_vec())
            == result.map_err(|error| vec![error])
    }

    fn validation_map_identity(value: i32, errors: Vec<String>) -> bool {
        let validation = match NonEmpty::from_vec(errors) {
            Some(errors) => Validation::Invalid(errors),
            None => Validation::Valid(value),
        };
        validation.clone().map(|x| x) == validation
    }

    fn validation_accumulates_in_order(first: String, rest: Vec<String>, last: String) -> bool {
        let left: Validation<(), String> = Validation::Invalid(NonEmpty::from_head_tail(first.clone(), rest.clone()));
        let right: Validation<(), String> = Validation::invalid(last.clone());
        let expected: Vec<_> = std::iter::once(first).chain(rest).chain(std::iter::once(last)).collect();
        left.zip(right).into_result().unwrap_err().into_vec() == expected
    }

    fn validation_zip_is_associative(a: Result<i16, String>, b: Result<i16, String>, c: Result<i16, String>) -> bool {
        let a = Validation::from(a);
        let b = Validation::from(b);
        let c = Validation::from(c);
        a.clone().zip(b.clone()).zip(c.clone()).map(|((a, b), c)| (a, b, c))
            == a.zip(b.zip(c)).map(|(a, (b, c))| (a, b, c))
    }

    fn reader_map_composes(environment: i16) -> bool {
        let composed = Reader::new(|r: &i16| i64::from(*r)).map(|x| x + 2).map(|x| x * 3);
        let direct = Reader::new(|r: &i16| (i64::from(*r) + 2) * 3);
        composed.run(&environment) == direct.run(&environment)
    }

    fn reader_and_then_shares_environment(environment: i16) -> bool {
        let reader = Reader::new(|r: &i16| i64::from(*r))
            .and_then(|first| Reader::new(move |r: &i16| first + i64::from(*r)));
        reader.run(&environment) == 2 * i64::from(environment)
    }

    fn reader_result_map_preserves_errors(result: Result<i16, String>) -> bool {
        let reader = ReaderResult::new(|_: &()| result.clone()).map(|x| i64::from(x) + 1);
        reader.run(&()) == result.clone().map(|x| i64::from(x) + 1)
    }

    fn reader_result_bind_matches_result(result: Result<i16, String>, environment: i16) -> bool {
        let reader = ReaderResult::new(|_: &i16| result.clone())
            .and_then(|value| ReaderResult::new(move |r: &i16| Ok(i64::from(value) + i64::from(*r))));
        reader.run(&environment) == result.clone().map(|value| i64::from(value) + i64::from(environment))
    }

    fn writer_map_preserves_events(value: i16, events: Vec<String>) -> bool {
        let writer = Writer::new(value, events.clone()).map(i64::from);
        writer.into_parts() == (i64::from(value), events)
    }

    fn writer_bind_is_associative(value: i16, events: Vec<String>) -> bool {
        let writer = Writer::new(i64::from(value), events);
        let f = |x| Writer::new(x + 1, ["increment".to_owned()]);
        let g = |x| Writer::new(x * 2, ["double".to_owned()]);
        writer.clone().and_then(f).and_then(g) == writer.and_then(|x| f(x).and_then(g))
    }

    fn writer_pure_is_a_bind_identity(value: i16, events: Vec<String>) -> bool {
        let writer = Writer::new(value, events);
        writer.clone().and_then(Writer::pure) == writer
            && Writer::pure(value).and_then(|_| writer.clone()) == writer
    }
}

#[test]
fn non_empty_access_and_iteration_preserve_the_invariant() {
    let mut values = NonEmpty::new(1);
    values.push(2);
    values.extend([3, 4]);
    assert_eq!(values.first(), &1);
    assert_eq!(values.last(), &4);
    assert_eq!(values.len(), 4);
    assert!(!values.is_empty());
    for value in &mut values {
        *value *= 2;
    }
    assert_eq!(values.iter().copied().collect::<Vec<_>>(), [2, 4, 6, 8]);
    assert_eq!(values.into_iter().rev().collect::<Vec<_>>(), [8, 6, 4, 2]);
}

#[test]
fn validation_combines_successes_and_keeps_errors_from_either_side() {
    assert_eq!(
        Validation::<_, &str>::Valid(2).map2(Validation::Valid(3), |a, b| a + b),
        Validation::Valid(5)
    );
    assert_eq!(
        Validation::<_, &str>::Valid(2).zip(Validation::<(), _>::invalid("right")),
        Validation::invalid("right")
    );
    assert_eq!(
        Validation::<(), _>::invalid("left").zip(Validation::<_, &str>::Valid(2)),
        Validation::invalid("left")
    );
}

#[test]
fn validation_maps_each_error_and_short_circuits_dependent_work() {
    let invalid: Validation<i32, _> = Validation::Invalid(NonEmpty::from_head_tail("a", ["b"]));
    assert_eq!(
        invalid.map_err(str::to_uppercase).into_result(),
        Err(NonEmpty::from_head_tail("A".to_owned(), ["B".to_owned()]))
    );
    let result = Validation::<i32, _>::invalid("bad").and_then(|_| panic!("must not run"));
    assert_eq!(result, Validation::<(), _>::invalid("bad"));
    assert_eq!(
        Validation::<_, &str>::Valid(2).and_then(|x| Validation::Valid(x + 1)),
        Validation::Valid(3)
    );
}

#[test]
fn reader_is_lazy_reusable_and_can_borrow_captures() {
    let calls = Cell::new(0);
    let offset = 5;
    let reader = Reader::new(|r: &i32| {
        calls.set(calls.get() + 1);
        r + offset
    });
    assert_eq!(calls.get(), 0);
    assert_eq!(reader.run(&2), 7);
    assert_eq!(reader.run(&3), 8);
    assert_eq!(calls.get(), 2);
}

#[test]
fn reader_supports_pure_ask_zip_and_local() {
    let reader = Reader::ask().zip(Reader::pure(3)).map(|(a, b)| a + b);
    let localized = reader.local(|text: &String| text.len());
    assert_eq!(localized.run(&"hello".to_owned()), 8);
}

#[test]
fn reader_result_short_circuits_and_recovers_with_the_same_environment() {
    let reader = ReaderResult::new(|_: &i32| Err::<i32, _>("missing"))
        .and_then(|_| panic!("must not run"))
        .or_else(|error| ReaderResult::new(move |r: &i32| Ok::<_, ()>(r + error.len() as i32)));
    assert_eq!(reader.run(&2), Ok(9));
}

#[test]
fn reader_result_zip_does_not_run_the_right_side_after_an_error() {
    let left = ReaderResult::new(|_: &()| Err::<i32, _>("left"));
    let right = ReaderResult::new(|_: &()| -> Result<i32, &str> { panic!("must not run") });
    assert_eq!(left.zip(right).run(&()), Err("left"));
}

#[test]
fn reader_result_composes_success_and_converts_to_and_from_reader() {
    let reader = ReaderResult::<usize, _, &str>::from_reader(Reader::pure(2))
        .and_then(|x| ReaderResult::new(move |r: &usize| Ok::<_, &str>(x + r)));
    assert_eq!(reader.into_reader().run(&1), Ok(3));
    let reader = ReaderResult::new(|r: &usize| Ok::<_, &str>(*r))
        .zip(ReaderResult::new(|_: &usize| Ok::<_, &str>(2)))
        .map(|(a, b)| a + b)
        .local(|text: &String| text.len());
    assert_eq!(reader.run(&"abc".to_owned()), Ok(5));
    assert_eq!(
        ReaderResult::new(|_: &()| Err::<(), _>("bad"))
            .map_err(str::len)
            .run(&()),
        Err(3)
    );
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Started { id: u32 },
    Finished { id: u32 },
}

#[test]
fn writer_defers_structured_logging_until_explicitly_written() {
    let captured = RefCell::new(Vec::new());
    let logger = |event| captured.borrow_mut().push(event);
    let writer = Writer::tell(Event::Started { id: 7 })
        .and_then(|()| Writer::pure(42))
        .zip(Writer::new("done", [Event::Finished { id: 7 }]));
    assert!(captured.borrow().is_empty());
    assert_eq!(writer.value(), &(42, "done"));
    assert_eq!(writer.events().len(), 2);
    let logger: &dyn Logger<Event> = &logger;
    assert_eq!(writer.write_to(logger), (42, "done"));
    assert_eq!(
        *captured.borrow(),
        [Event::Started { id: 7 }, Event::Finished { id: 7 }]
    );
}

#[test]
fn writer_maps_events_without_changing_the_value() {
    assert_eq!(
        Writer::new(5, [1, 2])
            .map_events(|x| x.to_string())
            .into_parts(),
        (5, vec!["1".to_owned(), "2".to_owned()])
    );
}
