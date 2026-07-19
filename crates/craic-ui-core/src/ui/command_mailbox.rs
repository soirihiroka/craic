use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static NEXT_HANDLER_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static HANDLERS: RefCell<HashMap<u64, Handler>> =
        RefCell::new(HashMap::new());
    static CANCELED_HANDLERS: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
}

struct Handler {
    callback: Box<dyn FnMut(Box<dyn Any + Send>)>,
    delivery: Delivery,
}

#[derive(Clone, Copy)]
enum Delivery {
    All,
    Latest,
    Once,
}

struct Mailbox {
    pending: Mutex<Vec<Box<dyn Any + Send>>>,
    scheduled: AtomicBool,
}

pub struct UiCommandSender<T> {
    handler_id: u64,
    mailbox: Arc<Mailbox>,
    marker: PhantomData<fn(T)>,
}

impl<T> Clone for UiCommandSender<T> {
    fn clone(&self) -> Self {
        Self {
            handler_id: self.handler_id,
            mailbox: self.mailbox.clone(),
            marker: PhantomData,
        }
    }
}

pub struct UiCommandSubscription {
    handler_id: u64,
    ui_thread: PhantomData<Rc<()>>,
}

impl Drop for UiCommandSubscription {
    fn drop(&mut self) {
        let removed =
            HANDLERS.with(|handlers| handlers.borrow_mut().remove(&self.handler_id).is_some());
        if !removed {
            CANCELED_HANDLERS.with(|canceled| {
                canceled.borrow_mut().insert(self.handler_id);
            });
        }
    }
}

pub fn channel<T, F>(handler: F) -> (UiCommandSender<T>, UiCommandSubscription)
where
    T: Send + 'static,
    F: FnMut(T) + 'static,
{
    let (sender, handler_id) = register(handler, Delivery::All);
    (
        sender,
        UiCommandSubscription {
            handler_id,
            ui_thread: PhantomData,
        },
    )
}

pub fn latest<T, F>(handler: F) -> (UiCommandSender<T>, UiCommandSubscription)
where
    T: Send + 'static,
    F: FnMut(T) + 'static,
{
    let (sender, handler_id) = register(handler, Delivery::Latest);
    (
        sender,
        UiCommandSubscription {
            handler_id,
            ui_thread: PhantomData,
        },
    )
}

pub fn once<T, F>(handler: F) -> UiCommandSender<T>
where
    T: Send + 'static,
    F: FnMut(T) + 'static,
{
    register(handler, Delivery::Once).0
}

fn register<T, F>(handler: F, delivery: Delivery) -> (UiCommandSender<T>, u64)
where
    T: Send + 'static,
    F: FnMut(T) + 'static,
{
    let handler_id = NEXT_HANDLER_ID.fetch_add(1, Ordering::Relaxed);
    let mut handler = handler;
    HANDLERS.with(|handlers| {
        handlers.borrow_mut().insert(
            handler_id,
            Handler {
                callback: Box::new(move |command| {
                    if let Ok(command) = command.downcast::<T>() {
                        handler(*command);
                    }
                }),
                delivery,
            },
        );
    });
    let mailbox = Arc::new(Mailbox {
        pending: Mutex::new(Vec::new()),
        scheduled: AtomicBool::new(false),
    });
    (
        UiCommandSender {
            handler_id,
            mailbox,
            marker: PhantomData,
        },
        handler_id,
    )
}

impl<T> UiCommandSender<T>
where
    T: Send + 'static,
{
    pub fn send(&self, command: T) {
        let Ok(mut pending) = self.mailbox.pending.lock() else {
            return;
        };
        pending.push(Box::new(command));
        drop(pending);

        if self
            .mailbox
            .scheduled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            schedule_drain(self.handler_id, self.mailbox.clone());
        }
    }
}

fn schedule_drain(handler_id: u64, mailbox: Arc<Mailbox>) {
    gtk::glib::idle_add_once(move || drain(handler_id, mailbox));
}

fn drain(handler_id: u64, mailbox: Arc<Mailbox>) {
    let commands = mailbox
        .pending
        .lock()
        .map(|mut pending| std::mem::take(&mut *pending))
        .unwrap_or_default();
    let Some(mut handler) = HANDLERS.with(|handlers| handlers.borrow_mut().remove(&handler_id))
    else {
        mailbox.scheduled.store(false, Ordering::Release);
        if let Ok(mut pending) = mailbox.pending.lock() {
            pending.clear();
        }
        return;
    };
    match handler.delivery {
        Delivery::Once => {
            if let Some(command) = commands.into_iter().next() {
                (handler.callback)(command);
            }
        }
        Delivery::Latest => {
            if let Some(command) = commands.into_iter().last() {
                (handler.callback)(command);
            }
        }
        Delivery::All => {
            for command in commands {
                (handler.callback)(command);
            }
        }
    }
    if !matches!(handler.delivery, Delivery::Once) {
        let canceled = CANCELED_HANDLERS.with(|canceled| canceled.borrow_mut().remove(&handler_id));
        if !canceled {
            HANDLERS.with(|handlers| {
                handlers.borrow_mut().insert(handler_id, handler);
            });
        }
    }

    mailbox.scheduled.store(false, Ordering::Release);
    let has_pending = mailbox
        .pending
        .lock()
        .is_ok_and(|pending| !pending.is_empty());
    if has_pending
        && mailbox
            .scheduled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    {
        schedule_drain(handler_id, mailbox);
    }
}
