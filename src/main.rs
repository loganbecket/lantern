mod decode;
mod grid;
mod loader;
mod photo;
mod thumbs;
mod window;

use adw::prelude::*;
use gtk::gio;

pub const APP_ID: &str = "io.github.loganbecket.Lantern";

fn main() -> gtk::glib::ExitCode {
    // Normally one instance handles every launch. With LANTERN_DEBUG set,
    // each launch is its own process so a test run doesn't open a window in
    // an already running Lantern.
    let mut flags = gio::ApplicationFlags::HANDLES_OPEN;
    if std::env::var_os("LANTERN_DEBUG").is_some() {
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::builder().application_id(APP_ID).flags(flags).build();
    app.connect_startup(|_| load_css());
    app.connect_activate(|app| window::build(app, None).present());
    // `lantern ~/Pictures`, or "Open with Lantern" from a file manager.
    app.connect_open(|app, files, _| {
        for file in files {
            window::build(app, Some(file.clone())).present();
        }
    });
    app.run()
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("no display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
