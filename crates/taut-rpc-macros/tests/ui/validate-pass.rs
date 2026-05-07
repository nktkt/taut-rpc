use taut_rpc::Validate;
#[derive(Validate)]
struct X {
    #[taut(length(min = 3, max = 32))]
    username: String,
    #[taut(min = 0, max = 100)]
    age: i32,
}
fn main() {
    let _ = X { username: String::new(), age: 0 };
}
