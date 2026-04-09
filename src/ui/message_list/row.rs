use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use super::item::MessageObject;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/justinf555/Epistle/ui/message_list/row.ui")]
    pub struct EpistleMessageRow {
        #[template_child]
        pub(super) avatar: TemplateChild<adw::Avatar>,
        #[template_child]
        pub(super) sender_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) time_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) subject_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) preview_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) star_icon: TemplateChild<gtk::Image>,

        pub(super) bindings: RefCell<Option<RowBindings>>,
    }

    impl std::fmt::Debug for EpistleMessageRow {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("EpistleMessageRow").finish()
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EpistleMessageRow {
        const NAME: &'static str = "EpistleMessageRow";
        type Type = super::EpistleMessageRow;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.set_layout_manager_type::<gtk::BinLayout>();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for EpistleMessageRow {
        fn dispose(&self) {
            self.dispose_template();
            if let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for EpistleMessageRow {}
}

glib::wrapper! {
    pub struct EpistleMessageRow(ObjectSubclass<imp::EpistleMessageRow>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl EpistleMessageRow {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Bind this row to a MessageObject. Updates all fields and wires
    /// property change signals for live updates.
    pub fn bind(&self, item: &MessageObject) {
        self.update_from(item);

        let row = self.clone();
        let read_handler = item.connect_is_read_notify(move |item| {
            row.update_read_style(item);
        });

        let row = self.clone();
        let flagged_handler = item.connect_is_flagged_notify(move |item| {
            row.update_flagged(item);
        });

        *self.imp().bindings.borrow_mut() = Some(RowBindings {
            item: item.downgrade(),
            read_handler,
            flagged_handler,
        });
    }

    /// Disconnect signals and reset visual state.
    pub fn unbind(&self) {
        let imp = self.imp();
        if let Some(b) = imp.bindings.borrow_mut().take() {
            if let Some(item) = b.item.upgrade() {
                item.disconnect(b.read_handler);
                item.disconnect(b.flagged_handler);
            }
        }
        imp.sender_label.set_text("");
        imp.subject_label.set_text("");
        imp.time_label.set_text("");
        imp.preview_label.set_text("");
        imp.preview_label.set_visible(false);
        imp.star_icon.set_visible(false);
        imp.avatar.set_text(None);
    }

    fn update_from(&self, item: &MessageObject) {
        let imp = self.imp();

        let sender_display = display_sender(item.sender().as_deref());
        imp.sender_label.set_text(&sender_display);
        imp.avatar.set_text(Some(&sender_display));

        imp.subject_label
            .set_text(item.subject().as_deref().unwrap_or("(no subject)"));

        imp.time_label
            .set_text(&format_timestamp(item.date().as_deref()));

        if let Some(preview) = item.preview() {
            imp.preview_label.set_text(&preview);
            imp.preview_label.set_visible(true);
        } else {
            imp.preview_label.set_visible(false);
        }

        self.update_read_style(item);
        self.update_flagged(item);
    }

    fn update_read_style(&self, item: &MessageObject) {
        let imp = self.imp();
        if item.is_read() {
            imp.sender_label.remove_css_class("heading");
            imp.subject_label.remove_css_class("heading");
        } else {
            imp.sender_label.add_css_class("heading");
            imp.subject_label.add_css_class("heading");
        }
    }

    fn update_flagged(&self, item: &MessageObject) {
        self.imp().star_icon.set_visible(item.is_flagged());
    }
}

/// Extract a display-friendly sender name.
fn display_sender(sender: Option<&str>) -> String {
    match sender {
        Some(s) => {
            if let Some(idx) = s.find(" <") {
                s[..idx].to_string()
            } else {
                s.to_string()
            }
        }
        None => "(unknown)".to_string(),
    }
}

/// Format a date string for the message list.
fn format_timestamp(date: Option<&str>) -> String {
    match date {
        Some(d) => {
            if d.len() > 16 {
                d[..16].to_string()
            } else {
                d.to_string()
            }
        }
        None => String::new(),
    }
}

/// Signal handler IDs stored during bind for explicit disconnect.
pub struct RowBindings {
    item: glib::WeakRef<MessageObject>,
    read_handler: glib::SignalHandlerId,
    flagged_handler: glib::SignalHandlerId,
}
