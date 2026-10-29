use std::{cell::RefCell, rc::Rc, time::Duration};

use gtk::prelude::*;
use javascriptcore::ValueExt;
use webkit2gtk::{LoadEvent, WebView, WebViewExt};

#[test]
#[ignore = "requires a desktop session"]
fn distinguishes_challenges_from_destination_content() {
    gtk::init().expect("A desktop session is required");
    let fixtures = [
        ("<title>example.mp4 :: Kwik</title><p>Ready</p>", true),
        ("<title>Just a moment...</title><button id='download'>Download</button>", false),
        ("<title>File</title><div id='challenge-stage'>Verify</div><video></video>", false),
        ("<title>File</title><input name='cf-turnstile-response' value=''><video></video>", true),
        ("<title>File</title><div class='cf-turnstile'>Verify</div>", false),
        ("<title>Generic site</title><h1>Welcome</h1>", true),
        ("<title>example.mp4 :: Kwik</title><div id='app'><form method='POST' action='/d/example'><button>Download</button></form></div>", true),
        ("<title>Just a moment...</title><p>Verification successful. Waiting for kwik.cx to respond</p>", false),
        ("<title>Just a moment...</title><p style='display:none'>Verification successful. Waiting for kwik.cx to respond</p>", false),
        ("<title>example.mp4 :: Kwik</title><div id='app'><form method='POST' action='/d/example'><button>Download</button></form></div><iframe src='about:blank' style='visibility:hidden'></iframe>", true),
        ("<title>File</title><input name='cf-turnstile-response' value='manually-solved'><video></video>", true),
        ("<title>AnimePahe</title><h1>Latest episodes</h1><a href='/anime'>Browse</a>", true),
        ("<title>AnimePahe</title><div class='g-recaptcha'>Verify</div>", false),
        ("<title>AnimePahe</title><div class='g-recaptcha'>Verified</div><textarea name='g-recaptcha-response'>manually-solved</textarea>", true),
        ("<title>AnimePahe</title><div class='h-captcha' style='display:none'>Inactive</div>", true),
    ];

    for (html, expected) in fixtures {
        let view = WebView::new();
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.add(&view);
        let actual = Rc::new(RefCell::new(None));
        let loaded_actual = actual.clone();
        view.connect_load_changed(move |view, event| {
            if event == LoadEvent::Finished {
                let actual = loaded_actual.clone();
                view.evaluate_javascript(
                    include_str!("../src/page_ready.js"),
                    None,
                    None,
                    None::<&gtk::gio::Cancellable>,
                    move |result| {
                        *actual.borrow_mut() = Some(result.map(|value| value.to_boolean()));
                        gtk::main_quit();
                    },
                );
            }
        });
        let timeout = gtk::glib::timeout_add_local(Duration::from_secs(10), || {
            gtk::main_quit();
            gtk::glib::ControlFlow::Continue
        });
        view.load_html(html, Some("https://animepahe.ru/"));
        window.show_all();
        gtk::main();
        timeout.remove();
        view.terminate_web_process();
        window.close();
        let actual = actual
            .borrow_mut()
            .take()
            .expect("Fixture timed out")
            .expect("JavaScript failed");
        assert_eq!(actual, expected, "Fixture: {html}");
    }
}
