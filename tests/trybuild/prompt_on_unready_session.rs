use bridge_core::session::Session;
use bridge_core::ids::SessionId;

fn main() {
    let s = Session::spawned(SessionId::parse("s").unwrap()); // Session<Spawned>
    let _ = s.send_prompt(vec![]); // ERROR: no method send_prompt on Session<Spawned>
}
