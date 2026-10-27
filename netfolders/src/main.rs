mod bookmarks;
mod mounts;
mod privileged;
mod smb;
mod ui;

fn main() -> gtk::glib::ExitCode {
    if std::env::args().nth(1).as_deref() == Some("--helper") {
        std::process::exit(privileged::helper_main());
    }
    ui::run()
}
