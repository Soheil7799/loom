//! A widget that draws with GTK's render nodes (on the GPU) through a callback: the chat's
//! tube, which spans the whole conversation. As a cairo DrawingArea it painted a surface that
//! tall (tens of MB in a long chat) on the CPU every time the chat changed height.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

type Draw = Box<dyn Fn(&Tube, &gtk::Snapshot)>;

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct Tube {
        pub draw: RefCell<Option<Draw>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Tube {
        const NAME: &'static str = "LoomTube";
        type Type = super::Tube;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Tube {}

    impl WidgetImpl for Tube {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(draw) = self.draw.borrow().as_ref() {
                draw(&self.obj(), snapshot);
            }
        }
    }
}

glib::wrapper! {
    pub struct Tube(ObjectSubclass<imp::Tube>) @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Tube {
    pub fn new() -> Tube {
        let tube: Tube = glib::Object::new();
        tube.set_can_target(false);
        tube
    }

    pub fn set_draw(&self, draw: impl Fn(&Tube, &gtk::Snapshot) + 'static) {
        *self.imp().draw.borrow_mut() = Some(Box::new(draw));
        self.queue_draw();
    }
}
