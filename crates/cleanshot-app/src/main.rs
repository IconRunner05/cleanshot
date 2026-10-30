#![forbid(unsafe_code)]

fn main() {
    cleanshot_editor::run(|cx| cx.quit()).expect("the Windows platform starts");
}
