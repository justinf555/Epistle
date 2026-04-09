use adw::prelude::*;

use super::item::MessageObject;
use super::row::EpistleMessageRow;

/// Build a `SignalListItemFactory` for the message list.
///
/// - **setup**: Creates a fresh `EpistleMessageRow` for each visible slot
/// - **bind**: Connects the row to a `MessageObject` from the model
/// - **unbind**: Disconnects signals and resets row state
/// - **teardown**: Removes the row widget
pub fn build_factory() -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();

    factory.connect_setup(|_, obj| {
        let list_item = obj.downcast_ref::<gtk::ListItem>().expect("is ListItem");
        let row = EpistleMessageRow::new();
        list_item.set_child(Some(&row));
    });

    factory.connect_bind(|_, obj| {
        let list_item = obj.downcast_ref::<gtk::ListItem>().expect("is ListItem");
        let row = list_item
            .child()
            .and_downcast::<EpistleMessageRow>()
            .expect("child is EpistleMessageRow");
        let item = list_item
            .item()
            .and_downcast::<MessageObject>()
            .expect("item is MessageObject");

        row.bind(&item);
    });

    factory.connect_unbind(|_, obj| {
        let list_item = obj.downcast_ref::<gtk::ListItem>().expect("is ListItem");
        let row = list_item
            .child()
            .and_downcast::<EpistleMessageRow>()
            .expect("child is EpistleMessageRow");

        row.unbind();
    });

    factory.connect_teardown(|_, obj| {
        let list_item = obj.downcast_ref::<gtk::ListItem>().expect("is ListItem");
        list_item.set_child(None::<&gtk::Widget>);
    });

    factory
}
