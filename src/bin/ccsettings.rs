#[path = "../CCConnect.rs"]
#[allow(non_snake_case)]
#[allow(dead_code)]
mod CCConnect;

fn main() {
    let resp = CCConnect::read_settings_for_frontend();
    CCConnect::write_settings_response(&resp);
}
