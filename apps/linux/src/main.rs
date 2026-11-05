//! `patch`: Patch for the Linux desktop.

mod icon;
mod layers;
mod notify;
mod screen;
mod shell;
mod signals;
mod snapshot;
mod tray;
mod widget;

use patch_linux::cli::{self, Args, Windowing};
use patch_linux::instance::{self, Primary, Request};
use patch_linux::paths::{Paths, APP_ID};
use patch_core::doc::{COL_TODO, DEFAULT_LIST};
use patch_core::time::now_local;
use patch_core::{ics, quick, Doc};
use patch_ui::App;
use shell::{Options, Shell};
use std::path::Path;
use std::process::ExitCode;
use std::rc::Rc;

fn main() -> ExitCode {
    let args = match cli::parse(std::env::args_os().skip(1)) {
        Ok(cli::Command::Run(args)) => args,
        Ok(cli::Command::Add { data_dir, text }) => return capture(Paths::from_env(data_dir), Request::Add(text)),
        Ok(cli::Command::Import { data_dir, file }) => return import(Paths::from_env(data_dir), &file),
        Ok(cli::Command::Version) => {
            println!("patch {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Ok(cli::Command::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("patch: {e}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };

    if let Err(e) = signals::install(|sig| {
        if !shell::post(|s| s.quit()) {
            std::process::exit(128 + sig);
        }
    }) {
        eprintln!("patch: can't handle signals: {e}");
    }

    let paths = Paths::from_env(args.data_dir.clone());
    if args.quit && !paths.lock_file().exists() {
        println!("Patch isn't running.");
        return ExitCode::SUCCESS;
    }
    let primary = match instance::acquire(&paths) {
        Ok(Some(primary)) => primary,
        Ok(None) => return forward(&paths, &args),
        Err(e) => {
            eprintln!("patch: can't start with data in {}: {e}", paths.data.display());
            return ExitCode::FAILURE;
        }
    };
    if args.quit {
        println!("Patch isn't running.");
        return ExitCode::SUCCESS;
    }
    match run(paths, args, primary) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("patch: {e}");
            ExitCode::FAILURE
        }
    }
}

fn import(paths: Paths, file: &Path) -> ExitCode {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("patch: can't read {}: {e}", file.display());
            return ExitCode::FAILURE;
        }
    };
    if !ics::is_calendar(&text) {
        eprintln!("patch: {} isn't an iCalendar file", file.display());
        return ExitCode::FAILURE;
    }
    // The running Patch reads the file itself, from wherever this was started.
    let path = std::fs::canonicalize(file).ok().and_then(|p| p.to_str().map(str::to_string)).filter(|p| !p.contains('\n'));
    let Some(path) = path else {
        eprintln!("patch: can't pass the path {} to Patch", file.display());
        return ExitCode::FAILURE;
    };
    capture(paths, Request::Import(path))
}

/// Hands new tasks to the running Patch, or writes them to its data when it
/// isn't running; they sync the next time it starts.
fn capture(paths: Paths, req: Request) -> ExitCode {
    let primary = match instance::acquire(&paths) {
        Ok(primary) => primary,
        Err(e) => {
            eprintln!("patch: can't use data in {}: {e}", paths.data.display());
            return ExitCode::FAILURE;
        }
    };
    let result = match primary {
        None => instance::send(&paths.socket(), req).map(|()| "Sent to Patch.".to_string()).map_err(|e| e.to_string()),
        Some(_lock) => write_offline(&paths.data.join("doc.json"), &req),
    };
    match result {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("patch: {e}");
            ExitCode::FAILURE
        }
    }
}

fn write_offline(file: &Path, req: &Request) -> Result<String, String> {
    let mut doc = Doc::load_or_new(file);
    let message = match req {
        Request::Add(text) => {
            let lists = doc.lists();
            let quick = quick::parse(text, now_local(), &lists);
            let list = lists.first().map_or(DEFAULT_LIST, |l| l.id.as_str());
            let task = quick.to_task(list, COL_TODO);
            doc.add_task(task);
            format!("Added \u{201c}{}\u{201d}.", quick.title)
        }
        Request::Import(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("can't read {path}: {e}"))?;
            format!("{}.", ics::import(&mut doc, &text, now_local()).summary())
        }
        _ => return Ok(String::new()),
    };
    doc.save(file).map_err(|e| format!("can't save {}: {e}", file.display()))?;
    Ok(message)
}

/// Another Patch owns this data directory: hand the request over and exit.
fn forward(paths: &Paths, args: &Args) -> ExitCode {
    if args.screenshot.is_some() {
        eprintln!("patch: Patch is running with this data directory; use --data-dir for screenshots");
        return ExitCode::FAILURE;
    }
    let req = if args.quit {
        Request::Quit
    } else if args.new_task {
        Request::New
    } else if args.background {
        Request::Ping
    } else {
        Request::Show
    };
    match instance::send(&paths.socket(), req) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("patch: Patch is running but doesn't answer ({e})");
            ExitCode::FAILURE
        }
    }
}

/// X11 when asked, or automatically whenever XWayland is available:
/// Wayland can't keep the pet above other windows or place the desk widget.
fn use_x11(windowing: Windowing) -> bool {
    let has = |var: &str| std::env::var_os(var).is_some_and(|v| !v.is_empty());
    match windowing {
        Windowing::X11 => true,
        Windowing::Wayland => false,
        Windowing::Auto => has("WAYLAND_DISPLAY") && has("DISPLAY"),
    }
}

fn select_backend(x11: bool) -> Result<(), slint::PlatformError> {
    let mut selector = slint::BackendSelector::new().backend_name("winit".into());
    if x11 {
        use slint::winit_030::winit::event_loop::EventLoop;
        use slint::winit_030::winit::platform::x11::EventLoopBuilderExtX11;
        let mut builder = EventLoop::<slint::winit_030::SlintEvent>::with_user_event();
        builder.with_x11();
        selector = selector.with_winit_event_loop_builder(builder);
    }
    selector.select()
}

fn run(paths: Paths, args: Args, mut primary: Primary) -> Result<(), String> {
    select_backend(use_x11(args.windowing))
        .map_err(|e| format!("can't open a window ({e}). On a machine without a desktop, run patch-hub instead."))?;
    slint::set_xdg_app_id(APP_ID).map_err(|e| e.to_string())?;

    let screenshot = args.screenshot.clone();
    let shell = Shell::new(paths.clone(), Options { windowing: args.windowing, headless_extras: screenshot.is_some() });
    let app = App::new(paths.data.clone(), shell.clone() as Rc<dyn patch_ui::Host>);
    shell.attach(&app).map_err(|e| format!("can't create the window: {e}"))?;

    match screenshot {
        Some(dir) => snapshot::start(&shell, dir).map_err(|e| format!("can't write screenshots: {e}"))?,
        None => {
            primary
                .serve(|req| {
                    shell::post(move |s| s.on_request(req));
                })
                .map_err(|e| format!("can't listen for other launches: {e}"))?;
            shell.start_tray();
            if args.new_task {
                shell.on_request(Request::New);
            } else if !args.background {
                shell.show_main();
            }
        }
    }

    let result = slint::run_event_loop_until_quit();
    shell.shutdown();
    drop(app);
    drop(primary);
    result.map_err(|e| e.to_string())
}
