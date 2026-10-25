//! A layout that keeps its child in a centred column no wider than a maximum: the chat reads
//! in lines of a comfortable length on a wide window, as in the chat apps people know.

use std::cell::Cell;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ColumnLayout {
        pub max: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ColumnLayout {
        const NAME: &'static str = "LoomColumnLayout";
        type Type = super::ColumnLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for ColumnLayout {}

    impl LayoutManagerImpl for ColumnLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, widget: &gtk::Widget, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = widget.first_child() else { return (0, 0, -1, -1) };
            // Height is asked for a width: the child gets no more than the column's.
            let for_size = if orientation == gtk::Orientation::Vertical && for_size > 0 { for_size.min(self.max.get()) } else { for_size };
            let (min, natural, _, _) = child.measure(orientation, for_size);
            // A scroller gives its child the least height it asks for. Content laid out taller
            // than that (a wrapped line, a picture) would hang past the end, out of reach: so
            // the height asked is the whole of it.
            if orientation == gtk::Orientation::Vertical { (natural, natural, -1, -1) } else { (min, natural, -1, -1) }
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, baseline: i32) {
            let Some(child) = widget.first_child() else { return };
            let (min, _, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
            let column = width.min(self.max.get()).max(min.min(width));
            let x = (width - column).max(0) / 2;
            child.size_allocate(&gtk::Allocation::new(x, 0, column, height), baseline);
        }
    }
}

glib::wrapper! {
    pub struct ColumnLayout(ObjectSubclass<imp::ColumnLayout>) @extends gtk::LayoutManager;
}

impl ColumnLayout {
    pub fn new(max: i32) -> ColumnLayout {
        let layout: ColumnLayout = glib::Object::new();
        layout.imp().max.set(max);
        layout
    }
}
