/* main.rs
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

mod application;
mod config;
mod ui;

use self::application::EpistleApplication;
use self::ui::EpistleWindow;

use config::{GETTEXT_PACKAGE, LOCALEDIR, PKGDATADIR};
use gettextrs::{bind_textdomain_codeset, bindtextdomain, textdomain};
use gtk::{gio, glib};
use gtk::prelude::*;
use tracing_subscriber::EnvFilter;

fn main() -> glib::ExitCode {
    // Set up tracing — default to info, override with RUST_LOG=epistle=debug
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("epistle=info")),
        )
        .init();

    // Set up gettext translations
    bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR).expect("Unable to bind the text domain");
    bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8")
        .expect("Unable to set the text domain encoding");
    textdomain(GETTEXT_PACKAGE).expect("Unable to switch to the text domain");

    // Load resources
    let resources = gio::Resource::load(PKGDATADIR.to_owned() + "/epistle.gresource")
        .expect("Could not load resources");
    gio::resources_register(&resources);

    // Build a Tokio runtime before the GTK main loop.
    let runtime = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
    let _guard = runtime.enter();

    // Build backend before GTK starts — main.rs is the composition root
    let engine = runtime.block_on(epistle::engine::MailEngine::open())
        .expect("Failed to initialize mail engine");

    let sync = runtime.block_on(epistle::sync::service::SyncEngine::new(
        engine.accounts(),
        engine.folders(),
        engine.messages(),
    )).expect("Failed to initialize sync engine");
    sync.start();

    // Pass engine to GTK app
    let app = EpistleApplication::new(
        "io.github.justinf555.Epistle",
        &gio::ApplicationFlags::empty(),
    );
    app.set_engine(engine);
    app.run()
}
