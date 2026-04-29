#[path = "../CCEverything.rs"]
#[allow(non_snake_case)]
#[allow(dead_code)]
mod CCEverything;

fn main() {
    let resp = CCEverything::run_from_stdin();
    CCEverything::write_json_response(&resp);
}
