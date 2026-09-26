use adw::prelude::*;

const APP_ID: &str = "io.github.loganbecket.Lantern";

fn main() -> gtk::glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &adw::Application) {
    let header = adw::HeaderBar::new();

    let empty = adw::StatusPage::builder()
        .icon_name("folder-pictures-symbolic")
        .title("Open a Folder")
        .description("Browse your photos with big thumbnails")
        .vexpand(true)
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&empty);

    adw::ApplicationWindow::builder()
        .application(app)
        .title("Lantern")
        .default_width(1200)
        .default_height(800)
        .content(&content)
        .build()
        .present();
}
