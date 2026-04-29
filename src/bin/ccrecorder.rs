#[path = "../ccrecorder/main.rs"]
#[allow(dead_code)]
mod ccrecorder_main;

fn main() {
    std::process::exit(ccrecorder_main::run_cli());
}