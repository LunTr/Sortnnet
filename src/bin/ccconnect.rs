#[path = "../CCConnect.rs"]
#[allow(non_snake_case)]
#[allow(dead_code)]
mod CCConnect;

fn main() {
    let resp = CCConnect::run_from_stdin();
    CCConnect::write_json_response(&resp);
}
