// A struct with `#[derive(Validate)]` and no `#[taut(...)]` constraints
// should produce an empty (always-Ok) Validate impl and compile cleanly.
use taut_rpc::Validate;

#[derive(Validate)]
struct Plain {
    name: String,
    age: u32,
}

fn main() {
    let _ = Plain {
        name: String::new(),
        age: 0,
    };
}
