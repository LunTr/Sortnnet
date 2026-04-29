#[path = "../CCGitConnect.rs"]
#[allow(non_snake_case)]
#[allow(dead_code)]
mod CCGitConnect;

fn main() {
    let resp = CCGitConnect::run_from_stdin();
    CCGitConnect::write_json_response(&resp);
}
