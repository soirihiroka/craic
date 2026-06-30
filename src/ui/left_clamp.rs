use adw::prelude::*;
use gtk::glib;
use gtk::glib::subclass::prelude::ObjectSubclassIsExt;
use std::cell::RefCell;

pub(in crate::ui) mod imp {
    use super::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub(in crate::ui) struct LeftClamp {
        pub(super) child: RefCell<Option<gtk::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LeftClamp {
        const NAME: &'static str = "CraicLeftClamp";
        type Type = super::LeftClamp;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for LeftClamp {
        fn dispose(&self) {
            if let Some(child) = self.child.borrow_mut().take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for LeftClamp {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self
                .child
                .borrow()
                .clone()
                .filter(|child| child.should_layout())
            else {
                return (0, 0, -1, -1);
            };

            if orientation == gtk::Orientation::Horizontal {
                (1, 1, -1, -1)
            } else {
                child.measure(orientation, for_size)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let Some(child) = self
                .child
                .borrow()
                .clone()
                .filter(|child| child.should_layout())
            else {
                return;
            };

            child.allocate(width, height, baseline, None);
        }
    }
}

glib::wrapper! {
    pub(in crate::ui) struct LeftClamp(ObjectSubclass<imp::LeftClamp>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl LeftClamp {
    pub(in crate::ui) fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let clamp: Self = glib::Object::builder().build();
        clamp.set_child(Some(child));
        clamp
    }

    pub(in crate::ui) fn set_child(&self, child: Option<&impl IsA<gtk::Widget>>) {
        let imp = self.imp();
        if let Some(existing) = imp.child.borrow_mut().take() {
            existing.unparent();
        }
        if let Some(child) = child {
            let child = child.as_ref().clone();
            child.set_parent(self);
            imp.child.replace(Some(child));
        }
        self.queue_resize();
    }
}
