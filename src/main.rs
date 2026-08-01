use gtk::prelude::*;
use magpie::ui::MagpieApplication;

fn main() -> gtk::glib::ExitCode {
    gtk::glib::set_application_name("Magpie");
    gtk::glib::set_prgname(Some(magpie::APP_ID));
    MagpieApplication::new().run()
}
