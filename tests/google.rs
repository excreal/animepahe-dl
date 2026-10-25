use std::{cell::RefCell, rc::Rc, time::Duration};

use gtk::prelude::*;
use webkit2gtk::{LoadEvent, WebView, WebViewExt};

#[test]
#[ignore = "requires a desktop session and internet access"]
fn opens_google() {
    gtk::init().expect("Could not initialize GTK; run this in a desktop session");
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_default_size(1024, 768);
    let webview = WebView::new();
    window.add(&webview);

    let result = Rc::new(RefCell::new(Err(
        "Google did not load with the expected page title".to_owned(),
    )));
    let loaded_result = result.clone();
    webview.connect_load_changed(move |view, event| {
        if event == LoadEvent::Finished {
            if view.title().is_some_and(|title| title.contains("Google")) {
                *loaded_result.borrow_mut() = Ok(());
            }
            gtk::main_quit();
        }
    });
    let failed_result = result.clone();
    webview.connect_load_failed(move |_, _, uri, error| {
        *failed_result.borrow_mut() = Err(format!("Failed to load {uri}: {error}"));
        gtk::main_quit();
        true
    });
    window.connect_delete_event(|_, _| {
        gtk::main_quit();
        gtk::glib::Propagation::Proceed
    });
    gtk::glib::timeout_add_local_once(Duration::from_secs(30), gtk::main_quit);

    webview.load_uri("https://www.google.com/");
    window.show_all();
    gtk::main();

    webview.terminate_web_process();
    window.close();
    assert!(result.borrow().is_ok(), "{:?}", result.borrow());
}
