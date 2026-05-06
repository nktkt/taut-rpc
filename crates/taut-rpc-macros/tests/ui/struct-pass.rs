use taut_rpc::Type;

#[derive(Type, serde::Serialize, serde::Deserialize)]
#[allow(dead_code)]
struct User {
    id: u64,
    name: String,
}

fn main() {}
