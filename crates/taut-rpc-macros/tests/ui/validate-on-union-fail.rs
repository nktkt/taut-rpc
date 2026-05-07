use taut_rpc::Validate;
#[derive(Validate)]
union U { a: u32, b: f32 }
fn main() {}
