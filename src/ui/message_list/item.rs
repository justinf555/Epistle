use std::cell::{Cell, RefCell};

use glib::prelude::*;
use glib::subclass::prelude::*;
use glib::Properties;
use gtk::glib;

use epistle::engine::traits::messages::Message;

mod imp {
    use super::*;

    #[derive(Default, Properties)]
    #[properties(wrapper_type = super::MessageObject)]
    pub struct MessageObject {
        #[property(get, set)]
        pub uid: Cell<u32>,
        #[property(get, set, nullable)]
        pub account_id: RefCell<Option<String>>,
        #[property(get, set, nullable)]
        pub folder_name: RefCell<Option<String>>,
        #[property(get, set, nullable)]
        pub sender: RefCell<Option<String>>,
        #[property(get, set, nullable)]
        pub subject: RefCell<Option<String>>,
        #[property(get, set, nullable)]
        pub date: RefCell<Option<String>>,
        #[property(get, set, nullable)]
        pub internal_date: RefCell<Option<String>>,
        #[property(get, set, nullable)]
        pub preview: RefCell<Option<String>>,
        #[property(get, set)]
        pub is_read: Cell<bool>,
        #[property(get, set)]
        pub is_flagged: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageObject {
        const NAME: &'static str = "EpistleMessageObject";
        type Type = super::MessageObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for MessageObject {}
}

glib::wrapper! {
    pub struct MessageObject(ObjectSubclass<imp::MessageObject>);
}

impl MessageObject {
    /// Create from a domain `Message`.
    pub fn new(msg: &Message) -> Self {
        let obj: Self = glib::Object::new();
        obj.set_uid(msg.uid);
        obj.set_account_id(Some(msg.account_id.clone()));
        obj.set_folder_name(Some(msg.folder_name.clone()));
        obj.set_sender(msg.sender.clone());
        obj.set_subject(msg.subject.clone());
        obj.set_date(msg.date.clone());
        obj.set_internal_date(msg.internal_date.clone());
        obj.set_preview(msg.preview.clone());
        obj.set_is_read(msg.is_read);
        obj.set_is_flagged(msg.is_flagged);
        obj
    }

    /// Update from a domain `Message` (re-sync from DB).
    pub fn update_from(&self, msg: &Message) {
        self.set_sender(msg.sender.clone());
        self.set_subject(msg.subject.clone());
        self.set_date(msg.date.clone());
        self.set_preview(msg.preview.clone());
        self.set_is_read(msg.is_read);
        self.set_is_flagged(msg.is_flagged);
    }
}
