//! The GPUI [`Platform`](gpui::Platform) that runs an app in the terminal.

use std::cell::RefCell;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, Mode, PostAction};
use gpui::{
    BackgroundExecutor, ClipboardItem, ForegroundExecutor, PlatformDisplay, PlatformKeyboardLayout,
    PlatformTextSystem,
};
use gpui_tui_term::{Session, SessionOptions};
use gpui_wgpu::CosmicTextSystem;

use crate::dispatcher::{RunnableQueue, TuiDispatcher};
use crate::display::TuiDisplay;
use crate::options::Options;
use crate::runtime::Runtime;

/// Fallback font family, the same GPUI's Linux backends use.
const FALLBACK_FONT_FAMILY: &str = "IBM Plex Sans";
/// Main-thread runnables run per event-loop wake-up before input gets a turn again.
const RUNNABLES_PER_WAKE: usize = 64;
/// How often to re-read the terminal size when it does not report resizes in-band.
const GEOMETRY_POLL_INTERVAL: Duration = Duration::from_millis(250);
const KEYBOARD_LAYOUT_NAME: &str = "terminal";

pub(crate) struct TuiPlatform {
    background_executor: BackgroundExecutor,
    foreground_executor: ForegroundExecutor,
    text_system: Arc<dyn PlatformTextSystem>,
    runtime: Rc<Runtime>,
    startup: RefCell<Option<Startup>>,
    state: RefCell<PlatformState>,
}

/// Parts moved into the event loop when the app starts running.
struct Startup {
    event_loop: EventLoop<'static, ()>,
    main_queue: RunnableQueue,
    main_ping_source: calloop::ping::PingSource,
    main_ping: calloop::ping::Ping,
    frame_ping_source: calloop::ping::PingSource,
}

#[derive(Default)]
struct PlatformState {
    clipboard: Option<ClipboardItem>,
    primary: Option<ClipboardItem>,
    quit_callback: Option<Box<dyn FnMut() -> bool>>,
}

impl TuiPlatform {
    /// Takes over the terminal. Fails if there is no terminal or it cannot show images.
    pub(crate) fn new(options: Options) -> Result<Self> {
        let session = Session::start(&SessionOptions {
            transport: options.transport,
            log_path: options.log_path.clone(),
            ..SessionOptions::default()
        })?;

        let event_loop = EventLoop::try_new().context("failed to create the event loop")?;
        let (main_ping, main_ping_source) = calloop::ping::make_ping()?;
        let (frame_ping, frame_ping_source) = calloop::ping::make_ping()?;
        let (dispatcher, main_queue) = TuiDispatcher::new(main_ping.clone())?;
        let dispatcher = Arc::new(dispatcher);

        let runtime = Runtime::new(
            session,
            options,
            event_loop.handle(),
            event_loop.get_signal(),
            frame_ping,
        );

        Ok(Self {
            background_executor: BackgroundExecutor::new(dispatcher.clone()),
            foreground_executor: ForegroundExecutor::new(dispatcher),
            text_system: Arc::new(CosmicTextSystem::new(FALLBACK_FONT_FAMILY)),
            runtime,
            startup: RefCell::new(Some(Startup {
                event_loop,
                main_queue,
                main_ping_source,
                main_ping,
                frame_ping_source,
            })),
            state: RefCell::new(PlatformState::default()),
        })
    }

    /// Registers the event sources and returns the loop, ready to run.
    fn prepare_event_loop(&self, startup: Startup) -> Result<EventLoop<'static, ()>> {
        let Startup {
            event_loop,
            mut main_queue,
            main_ping_source,
            main_ping,
            frame_ping_source,
        } = startup;
        let handle = event_loop.handle();

        handle
            .insert_source(main_ping_source, move |(), (), ()| {
                run_main_thread_tasks(&mut main_queue, &main_ping);
            })
            .map_err(|error| anyhow!("failed to watch the main-thread queue: {error}"))?;

        let runtime = Rc::downgrade(&self.runtime);
        handle
            .insert_source(frame_ping_source, move |(), (), ()| {
                if let Some(runtime) = runtime.upgrade() {
                    runtime.pump_frames();
                }
            })
            .map_err(|error| anyhow!("failed to watch frame requests: {error}"))?;

        let input = Generic::new(self.runtime.input_fd()?, Interest::READ, Mode::Level);
        let runtime = Rc::downgrade(&self.runtime);
        handle
            .insert_source(input, move |_, _, ()| {
                if let Some(runtime) = runtime.upgrade() {
                    runtime.handle_input_ready();
                }
                Ok(PostAction::Continue)
            })
            .map_err(|error| anyhow!("failed to watch terminal input: {error}"))?;

        if !self.runtime.has_in_band_resize() {
            let runtime = Rc::downgrade(&self.runtime);
            handle
                .insert_source(
                    Timer::from_duration(GEOMETRY_POLL_INTERVAL),
                    move |_, _, ()| {
                        let Some(runtime) = runtime.upgrade() else {
                            return TimeoutAction::Drop;
                        };
                        runtime.poll_geometry();
                        TimeoutAction::ToDuration(GEOMETRY_POLL_INTERVAL)
                    },
                )
                .map_err(|error| anyhow!("failed to poll the terminal size: {error}"))?;
        }
        Ok(event_loop)
    }

    fn display(&self) -> Rc<dyn PlatformDisplay> {
        Rc::new(TuiDisplay::new(
            self.runtime.content_size(),
            self.runtime.frame_interval(),
        ))
    }

    /// Opens files and URLs with the desktop, which only exists when running locally.
    fn open_with_desktop(&self, target: &std::ffi::OsStr) {
        if is_remote_session() {
            log::debug!("not opening {target:?}: running over SSH");
            return;
        }
        let spawned = Command::new("xdg-open")
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Err(error) = spawned {
            log::error!("failed to open {target:?} with xdg-open: {error}");
        }
    }
}

/// Runs queued main-thread work, a bounded amount per wake-up so input stays responsive.
fn run_main_thread_tasks(queue: &mut RunnableQueue, ping: &calloop::ping::Ping) {
    for _ in 0..RUNNABLES_PER_WAKE {
        match queue.try_pop() {
            Ok(Some(runnable)) => {
                runnable.run();
            }
            Ok(None) | Err(_) => return,
        }
    }
    // More work is waiting: come back after handling other events.
    ping.ping();
}

fn is_remote_session() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

struct TuiKeyboardLayout;

impl PlatformKeyboardLayout for TuiKeyboardLayout {
    fn id(&self) -> &str {
        KEYBOARD_LAYOUT_NAME
    }

    fn name(&self) -> &str {
        KEYBOARD_LAYOUT_NAME
    }
}

mod platform_impl;
