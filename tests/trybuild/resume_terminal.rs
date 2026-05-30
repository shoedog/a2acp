use bridge_core::task::Task;
use bridge_core::ids::TaskId;

fn main() {
    let done = Task::submitted(TaskId::parse("t").unwrap()).start().complete(); // Task<Terminal>
    let _ = done.resume(); // ERROR: no method resume on Task<Terminal>
}
