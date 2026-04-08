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

use epistle::engine::db::Database;
use epistle::goa::GoaClient;

use crate::config::VERSION;
use crate::EpistleWindow;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct EpistleApplication {
        pub(super) database: OnceCell<Database>,
        pub(super) goa_client: OnceCell<GoaClient>,
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
            let window = application.active_window().unwrap_or_else(|| {
                let window = EpistleWindow::new(&*application);
                window.upcast()
            });
            window.present();

            // Initialize engine on first activation
            if self.database.get().is_some() {
                return;
            }

            let app = application.clone();
            glib::MainContext::default().spawn_local(async move {
                if let Err(e) = app.init_engine().await {
                    eprintln!("Engine initialization failed: {e}");
                }
            });
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

    async fn init_engine(&self) -> anyhow::Result<()> {
        let db_path = glib::user_data_dir().join("epistle").join("mail.db");
        let db = Database::open(&db_path).await?;

        let mut goa = GoaClient::new().await?;
        let accounts = goa.discover_accounts().await?;

        for account in &accounts {
            db.upsert_account(account).await?;
        }

        eprintln!(
            "Discovered {} mail account(s){}",
            accounts.len(),
            if accounts.is_empty() {
                ". Add one in GNOME Settings → Online Accounts."
            } else {
                ""
            }
        );

        for account in &accounts {
            eprintln!("  • {} ({})", account.email_address, account.provider_name);
        }

        let imp = self.imp();
        imp.database.set(db).expect("database already initialized");
        imp.goa_client.set(goa).expect("goa_client already initialized");

        Ok(())
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
