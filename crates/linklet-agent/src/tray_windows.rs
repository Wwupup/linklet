//! Native Windows menu and dialogs. The network service runs on its own thread.
use linklet_agent::{Agent, AgentInfo, Invocation, Options, StartupError};
use rfd::{MessageDialog, MessageLevel};
use std::sync::mpsc;
use tao::{
    event::{Event, StartCause},
    event_loop::{ControlFlow, EventLoopBuilder},
};
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

enum UserEvent {
    Menu(MenuEvent),
    Ready(AgentInfo),
    Failed(StartupError),
    Stopped(Result<(), String>),
}

fn message(level: MessageLevel, text: &str) {
    MessageDialog::new()
        .set_title("Linklet")
        .set_description(text)
        .set_level(level)
        .show();
}

fn icon() -> Result<Icon, tray_icon::BadIcon> {
    let mut rgba = Vec::with_capacity(32 * 32 * 4);
    for y in 0..32 {
        for x in 0..32 {
            let color = if (8..13).contains(&x) && (6..26).contains(&y)
                || (8..25).contains(&x) && (21..26).contains(&y)
            {
                [255, 255, 255, 255]
            } else if !(2..30).contains(&x) || !(2..30).contains(&y) {
                [0, 0, 0, 0]
            } else {
                [22, 110, 180, 255]
            };
            rgba.extend_from_slice(&color);
        }
    }
    Icon::from_rgba(rgba, 32, 32)
}

fn connection_details(info: &AgentInfo) -> String {
    let addresses = match linklet_adapters::local_interfaces() {
        Ok(interfaces) => {
            let mut addresses: Vec<_> = interfaces
                .into_iter()
                .map(|interface| {
                    format!(
                        "{}:{}",
                        std::net::Ipv4Addr::from(interface.address),
                        info.port
                    )
                })
                .collect();
            addresses.sort();
            addresses.dedup();
            addresses.join("\n")
        }
        Err(error) => format!("LAN IPv4 addresses unavailable: {error}"),
    };
    format!(
        "Linklet {}\nListening on port {}\n\nLAN IPv4 addresses:\n{addresses}\n\nPairing code: {}\n\nRelative paths start in: {}\nLog: {}{}",
        env!("CARGO_PKG_VERSION"),
        info.port,
        info.pairing_code,
        info.base.display(),
        info.log_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "disabled".into()),
        if info.warnings.is_empty() {
            String::new()
        } else {
            format!("\n\n{}", info.warnings.join("\n"))
        }
    )
}

pub(super) fn run() {
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(Invocation::Start(options)) => options,
        Ok(Invocation::Help) => {
            message(
                MessageLevel::Info,
                &linklet_agent::USAGE.replace("linklet-agent", "linklet-tray"),
            );
            return;
        }
        Ok(Invocation::Version) => {
            message(
                MessageLevel::Info,
                &format!("linklet-tray {}", env!("CARGO_PKG_VERSION")),
            );
            return;
        }
        Err(error) => {
            message(MessageLevel::Error, &error.message);
            std::process::exit(error.exit_code);
        }
    };
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(UserEvent::Menu(event));
    }));

    let status = MenuItem::new("Starting...", false, None);
    let details = MenuItem::new("Connection details...", false, None);
    let copy_code = MenuItem::new("Copy pairing code", false, None);
    let copy_connection = MenuItem::new("Copy connection information", false, None);
    let logs = MenuItem::new("Open log folder", false, None);
    let quit = MenuItem::new("Exit", true, None);
    let menu = Menu::new();
    if let Err(error) = menu.append_items(&[
        &status,
        &PredefinedMenuItem::separator(),
        &details,
        &copy_code,
        &copy_connection,
        &logs,
        &PredefinedMenuItem::separator(),
        &quit,
    ]) {
        message(
            MessageLevel::Error,
            &format!("Cannot create tray menu: {error}"),
        );
        std::process::exit(1);
    }
    let (stop, shutdown) = mpsc::channel();
    let mut startup = Some((options, shutdown));
    let mut tray = None;
    let mut info: Option<AgentInfo> = None;
    let mut stopping = false;
    let service_proxy = event_loop.create_proxy();
    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                let result = icon().map_err(|error| error.to_string()).and_then(|icon| {
                    TrayIconBuilder::new()
                        .with_icon(icon)
                        .with_tooltip("Linklet: starting")
                        .with_menu(Box::new(menu.clone()))
                        .build()
                        .map_err(|error| error.to_string())
                });
                match result {
                    Ok(created) => tray = Some(created),
                    Err(error) => {
                        message(
                            MessageLevel::Error,
                            &format!("Cannot create tray icon: {error}"),
                        );
                        *control_flow = ControlFlow::ExitWithCode(1);
                        return;
                    }
                }
                if let Some((options, shutdown)) = startup.take() {
                    let proxy = service_proxy.clone();
                    let worker = std::thread::Builder::new()
                        .name("linklet-service".into())
                        .spawn(move || match Agent::start(options) {
                            Ok(agent) => {
                                if proxy
                                    .send_event(UserEvent::Ready(agent.info().clone()))
                                    .is_ok()
                                {
                                    let result = agent.serve(shutdown);
                                    let _ = proxy.send_event(UserEvent::Stopped(result));
                                }
                            }
                            Err(error) => {
                                let _ = proxy.send_event(UserEvent::Failed(error));
                            }
                        });
                    if let Err(error) = worker {
                        message(MessageLevel::Error, &format!("Cannot start agent: {error}"));
                        *control_flow = ControlFlow::ExitWithCode(1);
                    }
                }
            }
            Event::UserEvent(UserEvent::Ready(ready)) => {
                if !stopping {
                    status.set_text(format!("Running - port {}", ready.port));
                    copy_code.set_text(format!("Pairing code: {} (copy)", ready.pairing_code));
                    for item in [&details, &copy_code, &copy_connection] {
                        item.set_enabled(true);
                    }
                    logs.set_enabled(ready.log_path.is_some());
                    if let Some(tray) = &tray {
                        let _ = tray
                            .set_tooltip(Some(format!("Linklet: running on port {}", ready.port)));
                    }
                    if !ready.warnings.is_empty() {
                        message(MessageLevel::Warning, &ready.warnings.join("\n"));
                    }
                }
                info = Some(ready);
            }
            Event::UserEvent(UserEvent::Failed(error)) => {
                message(MessageLevel::Error, &error.message);
                *control_flow = ControlFlow::ExitWithCode(error.exit_code);
            }
            Event::UserEvent(UserEvent::Stopped(result)) => {
                let code = if let Err(error) = result {
                    message(MessageLevel::Error, &error);
                    1
                } else {
                    0
                };
                *control_flow = ControlFlow::ExitWithCode(code);
            }
            Event::UserEvent(UserEvent::Menu(event)) if !stopping => {
                if event.id == *quit.id() {
                    stopping = true;
                    status.set_text("Stopping - waiting for active requests...");
                    for item in [&details, &copy_code, &copy_connection, &logs, &quit] {
                        item.set_enabled(false);
                    }
                    if let Some(tray) = &tray {
                        let _ = tray.set_tooltip(Some("Linklet: stopping"));
                    }
                    let _ = stop.send(());
                } else if let Some(info) = &info {
                    if event.id == *details.id() {
                        message(MessageLevel::Info, &connection_details(info));
                    } else if event.id == *copy_code.id() || event.id == *copy_connection.id() {
                        let text = if event.id == *copy_code.id() {
                            info.pairing_code.clone()
                        } else {
                            connection_details(info)
                        };
                        if let Err(error) = clipboard_win::set_clipboard_string(&text) {
                            message(
                                MessageLevel::Error,
                                &format!("Cannot copy to clipboard: {error}"),
                            );
                        }
                    } else if event.id == *logs.id()
                        && let Some(path) = &info.log_path
                    {
                        let path = if path.is_absolute() {
                            path.clone()
                        } else {
                            info.base.join(path)
                        };
                        if let Some(directory) = path.parent()
                            && let Err(error) = std::process::Command::new("explorer.exe")
                                .arg(directory)
                                .spawn()
                        {
                            message(
                                MessageLevel::Error,
                                &format!("Cannot open log folder: {error}"),
                            );
                        }
                    }
                }
            }
            Event::LoopDestroyed => {
                let _ = stop.send(());
                drop(tray.take());
                MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
            }
            _ => {}
        }
    });
}
