// WebKit must be initialized on the process main thread, not a libtest worker.
#[allow(dead_code, unused_imports)]
#[path = "../src/main.rs"]
mod app;

fn main() {
    if !std::env::args().any(|arg| arg == "--ignored" || arg == "--include-ignored") {
        println!("Capture GUI regression skipped; run with --ignored in a desktop session.");
        return;
    }
    app::tests::captures_page_and_exits_without_shutdown_panic();
    app::tests::closes_browser_after_json_response();
    app::tests::saves_redirect_destination_cookies();
    println!("Capture, JSON completion, redirect cookies, and shutdown regressions passed.");
}
