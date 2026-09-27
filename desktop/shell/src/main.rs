//! The Bastion desktop shell (spec slice 3: BMD-15, BMD-27, BMD-28).
//!
//! A tray icon and one window that embeds the primary's web app (`/app`) in a
//! WebView — no chat or memory UI of its own (BMD-27). The tray has:
//! - **Open Bastion** — show/focus the window.
//! - **Stop node** — cut the local node now (BMD-15).
//! - **Quit** — close the shell (the supervised node dies with it).
//!
//! It reads `shell.json` (see [`config`]) for the primary URL and the paired
//! device token, which it injects into the WebView's localStorage so the app
//! starts signed in. Nothing here re-implements the interface.
//!
//! Built as a windowless app on Windows (`#![windows_subsystem = "windows"]`)
//! so launching it opens no console.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod node;

use std::sync::Arc;

use config::ShellConfig;
use node::NodeSupervisor;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIconBuilder, TrayIconEvent};
use wry::WebViewBuilder;

/// A JS string literal safe from injection: JSON-encode, so quotes and
/// backslashes in a token can never break out of the string.
fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn init_script(config: &ShellConfig) -> String {
    match &config.owner_token {
        Some(token) => format!(
            "try {{ localStorage.setItem('bastion.web.owner-token', {}); }} catch (e) {{}}",
            js_string(token)
        ),
        None => String::new(),
    }
}

fn main() -> anyhow::Result<()> {
    let config = ShellConfig::load().map_err(|e| {
        anyhow::anyhow!(
            "{e}\n\nWrite {} first (primary_url, owner_token) — see the README.",
            ShellConfig::path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "shell.json".into())
        )
    })?;
    let supervisor = Arc::new(NodeSupervisor::start(&config));

    let menu = Menu::new();
    let open = MenuItem::new("Open Bastion", true, None);
    let stop = MenuItem::new("Stop node", true, None);
    let quit = MenuItem::new("Quit", true, None);
    menu.append_items(&[&open, &stop, &quit])?;
    let (open_id, stop_id, quit_id) = (open.id().clone(), stop.id().clone(), quit.id().clone());

    let event_loop = EventLoopBuilder::new().build();
    let window = WindowBuilder::new()
        .with_title("Bastion")
        .with_inner_size(tao::dpi::LogicalSize::new(1100.0, 800.0))
        .build(&event_loop)?;

    let _webview = WebViewBuilder::new(&window)
        .with_url(&config.app_url())
        .with_initialization_script(&init_script(&config))
        .build()?;

    // The tray icon is created after the event loop exists so its menu events
    // reach the loop below.
    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Bastion")
        .build()?;

    let menu_channel = MenuEvent::receiver();
    let tray_channel = TrayIconEvent::receiver();
    let supervisor_for_loop = supervisor.clone();

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {}
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                // Closing the window hides to tray; Quit exits.
                window.set_visible(false);
            }
            _ => {}
        }
        while let Ok(menu_event) = menu_channel.try_recv() {
            if menu_event.id == open_id {
                window.set_visible(true);
                window.set_focus();
            } else if menu_event.id == stop_id {
                supervisor_for_loop.stop();
            } else if menu_event.id == quit_id {
                supervisor_for_loop.stop();
                *control_flow = ControlFlow::Exit;
            }
        }
        while let Ok(tray_event) = tray_channel.try_recv() {
            if let TrayIconEvent::DoubleClick { .. } = tray_event {
                window.set_visible(true);
                window.set_focus();
            }
        }
    });
}
