//! Task dispatch: worker threads for background work, a queue drained by the event loop for
//! main-thread work, and a timer thread for delayed work.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::{Duration, Instant};

use calloop::ping::Ping;
use gpui::{
    PlatformDispatcher, Priority, PriorityQueueReceiver, PriorityQueueSender, RunnableVariant,
};

const MIN_WORKER_THREADS: usize = 2;

/// The receiving end of a runnable queue. The main-thread one is drained by the event loop.
pub(crate) type RunnableQueue = PriorityQueueReceiver<RunnableVariant>;

pub(crate) struct TuiDispatcher {
    main_sender: PriorityQueueSender<RunnableVariant>,
    /// Wakes the event loop when main-thread work arrives.
    main_ping: Ping,
    background_sender: PriorityQueueSender<RunnableVariant>,
    timer_sender: mpsc::Sender<DelayedRunnable>,
    main_thread_id: ThreadId,
    _threads: Vec<JoinHandle<()>>,
}

impl TuiDispatcher {
    /// Must be called on the main thread.
    pub(crate) fn new(main_ping: Ping) -> anyhow::Result<(Self, RunnableQueue)> {
        let (main_sender, main_queue) = PriorityQueueReceiver::new();
        let (background_sender, background_queue) = PriorityQueueReceiver::new();
        let worker_count = thread::available_parallelism().map_or(MIN_WORKER_THREADS, |count| {
            count.get().max(MIN_WORKER_THREADS)
        });

        let mut threads = (0..worker_count)
            .map(|index| spawn_worker(index, background_queue.clone()))
            .collect::<std::io::Result<Vec<_>>>()?;
        let (timer_sender, timer_receiver) = mpsc::channel();
        threads.push(spawn_timer_thread(timer_receiver)?);

        let dispatcher = Self {
            main_sender,
            main_ping,
            background_sender,
            timer_sender,
            main_thread_id: thread::current().id(),
            _threads: threads,
        };
        Ok((dispatcher, main_queue))
    }
}

impl PlatformDispatcher for TuiDispatcher {
    fn is_main_thread(&self) -> bool {
        thread::current().id() == self.main_thread_id
    }

    fn dispatch(&self, runnable: RunnableVariant, priority: Priority) {
        if let Err(error) = self.background_sender.send(priority, runnable) {
            // The workers are gone, so the app is shutting down. Dropping a runnable on the
            // wrong thread is unsafe for !Send futures; leak it instead.
            std::mem::forget(error);
        }
    }

    fn dispatch_on_main_thread(&self, runnable: RunnableVariant, priority: Priority) {
        match self.main_sender.send(priority, runnable) {
            Ok(()) => self.main_ping.ping(),
            // The event loop is gone. The runnable may wrap a !Send future, so it must not be
            // dropped on this (possibly background) thread.
            Err(error) => std::mem::forget(error),
        }
    }

    fn dispatch_after(&self, duration: Duration, runnable: RunnableVariant) {
        let delayed = DelayedRunnable {
            due: Instant::now() + duration,
            runnable,
        };
        if let Err(error) = self.timer_sender.send(delayed) {
            // Dropping would cancel the task and panic its awaiter; leaking leaves it pending,
            // which is fine during shutdown.
            std::mem::forget(error);
        }
    }

    fn spawn_realtime(&self, function: Box<dyn FnOnce() + Send>) {
        if let Err(error) = thread::Builder::new()
            .name("gpui-tui-realtime".to_owned())
            .spawn(function)
        {
            log::error!("failed to spawn a realtime thread: {error}");
        }
    }
}

fn spawn_worker(index: usize, queue: RunnableQueue) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name(format!("gpui-tui-worker-{index}"))
        .spawn(move || {
            for runnable in queue.iter() {
                runnable.run();
            }
        })
}

struct DelayedRunnable {
    due: Instant,
    runnable: RunnableVariant,
}

impl PartialEq for DelayedRunnable {
    fn eq(&self, other: &Self) -> bool {
        self.due == other.due
    }
}

impl Eq for DelayedRunnable {}

impl PartialOrd for DelayedRunnable {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DelayedRunnable {
    /// Reversed, so that `BinaryHeap` pops the earliest deadline first.
    fn cmp(&self, other: &Self) -> Ordering {
        other.due.cmp(&self.due)
    }
}

/// Runs delayed runnables when they are due. They are background work, so any thread will do.
fn spawn_timer_thread(
    receiver: mpsc::Receiver<DelayedRunnable>,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("gpui-tui-timer".to_owned())
        .spawn(move || {
            let mut pending = BinaryHeap::new();
            loop {
                let now = Instant::now();
                while pending
                    .peek()
                    .is_some_and(|next: &DelayedRunnable| next.due <= now)
                {
                    if let Some(due) = pending.pop() {
                        due.runnable.run();
                    }
                }

                let received = match pending.peek() {
                    Some(next) => receiver.recv_timeout(next.due.saturating_duration_since(now)),
                    None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                match received {
                    Ok(delayed) => pending.push(delayed),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        // Leak what is left rather than cancelling tasks during shutdown.
                        std::mem::forget(pending);
                        return;
                    }
                }
            }
        })
}
