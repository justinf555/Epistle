/* application.rs
 *
 * Copyright 2026 Unknown
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

use std::cell::OnceCell;

use gettextrs::gettext;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use epistle::app_event::AppEvent;
use epistle::engine::MailEngine;

use crate::config::VERSION;
use crate::EpistleWindow;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct EpistleApplication {
        pub(super) engine: OnceCell<MailEngine>,
        pub(super) activated: std::cell::Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EpistleApplication {
        const NAME: &'static str = "EpistleApplication";
        type Type = super::EpistleApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for EpistleApplication {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.setup_gactions();
            obj.set_accels_for_action("app.quit", &["<control>q"]);
        }
    }

    impl ApplicationImpl for EpistleApplication {
        fn activate(&self) {
            let application = self.obj();
            let engine = self.engine.get().expect("engine set before activate");

            let window = application.active_window().unwrap_or_else(|| {
                let window = EpistleWindow::new(&*application);
                window.set_engine(engine.accounts(), engine.folders());
                window.upcast()
            });
            window.present();

            // Emit AppStarted once — SyncEngine reacts
            if !self.activated.get() {
                self.activated.set(true);
                engine.sender().send(AppEvent::AppStarted);
            }
        }

        fn shutdown(&self) {
            if let Some(engine) = self.engine.get() {
                engine.sender().send(AppEvent::AppShutdown);
            }
            self.parent_shutdown();
        }
    }

    impl GtkApplicationImpl for EpistleApplication {}
    impl AdwApplicationImpl for EpistleApplication {}
}

glib::wrapper! {
    pub struct EpistleApplication(ObjectSubclass<imp::EpistleApplication>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl EpistleApplication {
    pub fn new(application_id: &str, flags: &gio::ApplicationFlags) -> Self {
        glib::Object::builder()
            .property("application-id", application_id)
            .property("flags", flags)
            .property("resource-base-path", "/io/github/justinf555/Epistle")
            .build()
    }

    /// Inject the mail engine, built by main.rs before GTK starts.
    pub fn set_engine(&self, engine: MailEngine) {
        self.imp().engine.set(engine).ok().expect("engine set once");
    }

    fn setup_gactions(&self) {
        let quit_action = gio::ActionEntry::builder("quit")
            .activate(move |app: &Self, _, _| app.quit())
            .build();
        let about_action = gio::ActionEntry::builder("about")
            .activate(move |app: &Self, _, _| app.show_about())
            .build();
        self.add_action_entries([quit_action, about_action]);
    }

    fn show_about(&self) {
        let window = self.active_window().unwrap();
        let about = adw::AboutDialog::builder()
            .application_name("Epistle")
            .application_icon("io.github.justinf555.Epistle")
            .developer_name("Unknown")
            .version(VERSION)
            .developers(vec!["Unknown"])
            // Translators: Replace "translator-credits" with your name/username, and optionally an email or URL.
            .translator_credits(&gettext("translator-credits"))
            .copyright("© 2026 Unknown")
            .build();

        about.present(Some(&window));
    }
}
