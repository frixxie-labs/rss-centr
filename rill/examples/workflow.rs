use rillfp::{Reader, ReaderResult, Validation, Writer};

struct Config {
    enabled: bool,
    greeting: String,
}

#[derive(Debug)]
struct Greeted {
    name: String,
}

fn validate(name: &str, age: u8) -> Validation<(String, u8), &'static str> {
    let name = Validation::from(
        (!name.trim().is_empty())
            .then(|| name.trim().to_owned())
            .ok_or("name is required"),
    );
    let age = Validation::from((age > 0).then_some(age).ok_or("age must be positive"));
    name.zip(age)
}

fn main() -> Result<(), &'static str> {
    let errors = validate("", 0).into_result().unwrap_err();
    assert_eq!(errors.len(), 2);

    let (name, _age) = validate("Ada", 37)
        .into_result()
        .map_err(|_| "invalid input")?;
    let config = Config {
        enabled: true,
        greeting: "Hello".to_owned(),
    };

    let computation = ReaderResult::new(|config: &Config| {
        config.enabled.then_some(()).ok_or("greetings disabled")
    })
    .and_then(|()| {
        ReaderResult::from_reader(Reader::new(|config: &Config| {
            Writer::new(
                format!("{}, {}!", config.greeting, name),
                [Greeted { name: name.clone() }],
            )
        }))
    });

    let writer = computation.run(&config)?;
    let logger = |event: Greeted| println!("greeted name={:?}", event.name);
    println!("{}", writer.write_to(&logger));
    Ok(())
}
