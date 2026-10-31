use std::cell::RefCell;

use quickcheck::quickcheck;
use rillfp::{NonEmpty, Reader, ReaderResult, Validation, Writer};

quickcheck! {
    fn reader_maps_optional_values_like_option(value: Option<i16>) -> bool {
        let reader = Reader::new(|environment: &Option<i16>| *environment)
            .map(|value| value.map(|number| i64::from(number) + 1));

        reader.run(&value) == value.map(|number| i64::from(number) + 1)
    }

    fn reader_result_preserves_optional_success_and_errors(value: Result<Option<i16>, String>) -> bool {
        let reader = ReaderResult::new(|environment: &Result<Option<i16>, String>| environment.clone())
            .map(|value| value.map(|number| i64::from(number) + 1));

        reader.run(&value) == value.clone().map(|value| value.map(|number| i64::from(number) + 1))
    }

    fn writer_maps_result_without_losing_events(value: Result<i16, String>, events: Vec<String>) -> bool {
        let writer = Writer::new(value.clone(), events.clone())
            .map(|result| result.map(|number| i64::from(number) + 1));

        writer.into_parts() == (value.map(|number| i64::from(number) + 1), events)
    }

    fn writer_maps_option_without_losing_events(value: Option<i16>, events: Vec<String>) -> bool {
        let writer = Writer::new(value, events.clone())
            .map(|option| option.map(|number| i64::from(number) + 1));

        writer.into_parts() == (value.map(|number| i64::from(number) + 1), events)
    }
}

#[test]
fn validation_accumulates_errors_from_option_and_result_inputs() {
    for (name, age, expected) in [
        (Some("Ada"), "37", Ok(("Ada", 37))),
        (None, "37", Err(vec!["missing name"])),
        (Some("Ada"), "invalid", Err(vec!["invalid age"])),
        (None, "invalid", Err(vec!["missing name", "invalid age"])),
    ] {
        let name = Validation::from(name.ok_or("missing name"));
        let age = Validation::from(age.parse::<u8>().map_err(|_| "invalid age"));

        let result = name.zip(age).into_result().map_err(NonEmpty::into_vec);

        assert_eq!(result, expected);
    }
}

#[test]
fn reader_can_transpose_an_optional_result_before_using_reader_result() {
    let reader =
        Reader::new(|input: &Option<&str>| input.map(str::parse::<u16>)).map(Option::transpose);
    let reader = ReaderResult::new(|input| reader.run(input)).map_err(|_| "invalid number");

    assert_eq!(reader.run(&Some("42")), Ok(Some(42)));
    assert_eq!(reader.run(&None), Ok(None));
    assert_eq!(reader.run(&Some("invalid")), Err("invalid number"));
}

#[test]
fn reader_result_supports_question_mark_with_required_options_and_results() {
    let reader = ReaderResult::new(|input: &Option<&str>| {
        let text = input.ok_or("missing number")?;
        let number = text.parse::<u16>().map_err(|_| "invalid number")?;
        Ok(number)
    })
    .and_then(|number| {
        ReaderResult::new(move |_: &Option<&str>| number.checked_add(1).ok_or("overflow"))
    });

    assert_eq!(reader.run(&Some("41")), Ok(42));
    assert_eq!(reader.run(&None), Err("missing number"));
    assert_eq!(reader.run(&Some("invalid")), Err("invalid number"));
    assert_eq!(reader.run(&Some("65535")), Err("overflow"));
}

#[test]
fn non_empty_optional_elements_use_standard_option_collection() {
    for (values, expected) in [
        (
            NonEmpty::from_head_tail(Some(1), [Some(2)]),
            Some(vec![1, 2]),
        ),
        (NonEmpty::from_head_tail(None, [Some(2)]), None),
        (NonEmpty::from_head_tail(Some(1), [None]), None),
    ] {
        let collected: Option<Vec<_>> = values.into_iter().collect();
        let non_empty = collected.and_then(NonEmpty::from_vec);

        assert_eq!(non_empty.map(NonEmpty::into_vec), expected);
    }
}

#[test]
fn non_empty_fallible_elements_use_standard_result_collection() {
    for (values, expected) in [
        (NonEmpty::from_head_tail(Ok(1), [Ok(2)]), Ok(vec![1, 2])),
        (
            NonEmpty::from_head_tail(Err("first"), [Ok(2)]),
            Err("first"),
        ),
        (
            NonEmpty::from_head_tail(Ok(1), [Err("second")]),
            Err("second"),
        ),
        (
            NonEmpty::from_head_tail(Err("first"), [Err("second")]),
            Err("first"),
        ),
    ] {
        let collected: Result<Vec<_>, _> = values.into_iter().collect();

        assert_eq!(collected, expected);
    }
}

#[test]
fn writer_emits_events_even_when_its_result_is_an_error() {
    for value in [Ok(42), Err("failed")] {
        let events = RefCell::new(Vec::new());
        let logger = |event| events.borrow_mut().push(event);
        let writer =
            Writer::new(value, ["attempted"]).and_then(|result| Writer::new(result, ["finished"]));

        assert!(events.borrow().is_empty());
        assert_eq!(writer.write_to(&logger), value);
        assert_eq!(*events.borrow(), ["attempted", "finished"]);
    }
}

#[test]
fn writer_emits_events_even_when_its_option_is_none() {
    for value in [Some(42), None] {
        let events = RefCell::new(Vec::new());
        let logger = |event| events.borrow_mut().push(event);
        let writer = Writer::new(value, ["lookup attempted"]);

        assert!(events.borrow().is_empty());
        assert_eq!(writer.write_to(&logger), value);
        assert_eq!(*events.borrow(), ["lookup attempted"]);
    }
}
