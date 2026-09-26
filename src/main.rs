mod decode;
mod grid;
mod loader;
mod thumbs;
mod window;

use adw::prelude::*;
use gtk::gio;

pub const APP_ID: &str = "io.github.loganbecket.Lantern";

fn main() -> gtk::glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
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
