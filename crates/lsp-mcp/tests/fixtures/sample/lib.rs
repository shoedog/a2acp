pub fn add(a: i32, b: i32) -> i32 {
    a + b
}
pub fn caller() -> i32 {
    add(1, 2)
}
pub trait Greet {
    fn hi(&self) -> &'static str;
}
pub struct En;
impl Greet for En {
    fn hi(&self) -> &'static str {
        "hi"
    }
}
