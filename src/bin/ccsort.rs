#[path = "../CCSort.rs"]
#[allow(non_snake_case)]
#[allow(dead_code)]
mod CCSort;

fn main() {
    let resp = CCSort::run_from_stdin();
    CCSort::write_json_response(&resp);
}
