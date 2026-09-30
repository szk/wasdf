//! The event loop: render, wait for one AppEvent, dispatch it, then run any
//! pending Suspended execution. It contains no intent-specific branches — every
//! key decision goes through the kernel's single pipeline.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::{self, Event};

use crate::app::kernel::App;
use crate::app::terminal;
use crate::core::AppEvent;

pub fn run(cwd: PathBuf) -> io::Result<()> {
    let mut term = terminal::init();
    let mut app = App::boot(cwd);

    // While a Suspended child (the editor) runs in the foreground, the input
    // thread must not also read stdin — otherwise it steals the editor's input
    // and buffers every key typed in it, which then floods the app on return.
    let input_paused = Arc::new(AtomicBool::new(false));

    // Input thread: convert crossterm events into AppEvents (idle while paused).
    let tx = app.tx.clone();
    let paused = input_paused.clone();
    std::thread::spawn(move || loop {
        if paused.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }
        match event::poll(Duration::from_millis(250)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(k)) => {
                    if let Some(key) = terminal::convert_key(k) {
                        if tx.send(AppEvent::Key(key)).is_err() {
                            break;
                        }
                    }
                }
                Ok(Event::Resize(w, h)) => {
                    let _ = tx.send(AppEvent::Resize(w, h));
                }
                Ok(_) => {}
                Err(_) => break,
            },
            Ok(false) => {
                if tx.send(AppEvent::Tick).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    });

    let result = loop {
        // Surface any diagnostic that reached the @/messages log without
        // going through a notice (boot config warnings, loader failures) —
        // before the draw, so it shows this frame, and on every pass
        // (including Tick), so boot diagnostics appear at launch.
        app.surface_new_messages();
        let top_opts = {
            let s = &app.store;
            crate::ui::top::TopOptions {
                border_style: s.get_str("border-style"),
                monitor: crate::ui::top::MonitorOptions::from_store(s),
                scrollbar: crate::ui::chrome::ScrollbarChars {
                    thumb: s.get_str("scrollbar-thumb"),
                    rail: s.get_str("scrollbar-rail"),
                    disabled: s.get_str("scrollbar-disabled"),
                },
                file_row_size_graph: s.get_bool("file-row-size-graph"),
            }
        };
        let theme = app.theme_registry.read().unwrap().resolve_active();
        let view = crate::ui::render::View {
            layouts: &app.layouts,
            options: &app.store,
            keymap: &app.keymap,
            conds: &app.conditions,
        };
        if let Err(e) = term.draw(|f| app.ui.render(f, &app.state, &app.extensions, &app.vfs, &top_opts, &view, &theme)) {
            break Err(e);
        }
        // The reducer advances function scroll/hscroll blindly (it can't see the
        // viewport); feed the render-measured bounds back into each kernel-owned
        // Scroll: bind folds in the bound and clamps the offset, so over-scroll is
        // discarded and reversing direction moves immediately.
        app.state.function.scroll.bind(app.ui.function_max_scroll());
        app.state.function.hscroll.bind(app.ui.function_hmax_scroll());
        // Refresh the file-list geometry the renderer just measured, so the
        // reducer's 2-D cursor movement matches what is on screen.
        app.state.list_geom = app.ui.list_geom();
        if app.should_quit() {
            break Ok(());
        }
        match app.rx.recv() {
            Ok(AppEvent::Key(k)) => app.handle_key(k),
            Ok(AppEvent::Async(r)) => app.handle_async(r),
            Ok(AppEvent::Resize(_, _)) | Ok(AppEvent::Tick) => {}
            Err(_) => break Ok(()),
        }
        // After the event, never before: an extension reacting to something
        // that finished on its own (a PTY child exiting) must see the state
        // this pass produced.
        app.tick();
        if let Some(argv) = app.take_suspend() {
            let cwd = app.cwd();
            // Hand stdin to the editor: pause the input thread, run it, then drain
            // any events that slipped through (so the keys typed in the editor do
            // not flood the app), and resume.
            input_paused.store(true, Ordering::Relaxed);
            let suspended = terminal::run_suspended(&argv, &cwd);
            while app.rx.try_recv().is_ok() {}
            input_paused.store(false, Ordering::Relaxed);
            match suspended {
                Ok(t) => term = t,
                Err(e) => break Err(e),
            }
            app.after_suspend();
        }
    };

    terminal::restore();
    result
}
