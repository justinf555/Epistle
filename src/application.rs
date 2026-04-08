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
use std::sync::Arc;

use gettextrs::gettext;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use epistle::app_event::AppEvent;
use epistle::engine::accounts::MailAccountsImpl;
use epistle::engine::db::Database;
use epistle::engine::folders::MailFoldersImpl;
use epistle::event_bus::{EventBus, EventSender};
use epistle::goa::GoaClient;
use epistle::sync::service::SyncEngine;

use crate::config::VERSION;
use crate::EpistleWindow;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct EpistleApplication {
        pub(super) event_bus: OnceCell<EventBus>,
        pub(super) initialized: std::cell::Cell<bool>,
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

            // Create the event bus once, before the window
            if self.event_bus.get().is_none() {
                let bus = EventBus::new();
                self.event_bus.set(bus).expect("event_bus set once");
            }
            let bus = self.event_bus.get().unwrap();

            let window = application.active_window().unwrap_or_else(|| {
                let window = EpistleWindow::new(&*application);
                window.subscribe_events(bus);
                window.upcast()
            });
            window.present();

            // Wire up engine + sync on first activation only
            if self.initialized.get() {
                return;
            }
            self.initialized.set(true);

            let sender = bus.sender();
            glib::MainContext::default().spawn_local(async move {
                if let Err(e) = init_engine(sender).await {
                    eprintln!("Engine initialization failed: {e}");
                }
            });
        }
    }

    impl GtkApplicationImpl for EpistleApplication {}
    impl AdwApplicationImpl for EpistleApplication {}
}

/// Create engine storage impls, emit cached data, wire up SyncEngine, and fire AppStarted.
async fn init_engine(sender: EventSender) -> anyhow::Result<()> {
    use epistle::engine::traits::accounts::MailAccounts;
    use epistle::engine::traits::folders::MailFolders;

    let db_path = glib::user_data_dir().join("epistle").join("mail.db");
    let db = Database::open(&db_path).await?;

    // Domain-pure engine storage
    let accounts = Arc::new(MailAccountsImpl::new(db.clone(), sender.clone()));
    let folders = Arc::new(MailFoldersImpl::new(db, sender.clone()));

    // Show cached data immediately — sidebar populates before IMAP finishes
    let cached_accounts = accounts.list_accounts().await?;
    if !cached_accounts.is_empty() {
        // Accounts first so sidebar creates sections, then folders fill them
        sender.send(AppEvent::AccountsChanged {
            accounts: cached_accounts.clone(),
        });
        for account in &cached_accounts {
            let cached_folders = folders.list_folders(&account.goa_id).await?;
            if !cached_folders.is_empty() {
                sender.send(AppEvent::FoldersChanged {
                    account_id: account.goa_id.clone(),
                    email_address: account.email_address.clone(),
                    folders: cached_folders,
                });
            }
        }
    }

    // Sync engine — owns GOA, writes into engine via trait objects
    let goa = GoaClient::new().await?;
    let sync = SyncEngine::new(goa, accounts, folders);
    sync.subscribe();

    // Fire lifecycle event — SyncEngine reacts (IMAP runs in background)
    sender.send(AppEvent::AppStarted);

    Ok(())
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
