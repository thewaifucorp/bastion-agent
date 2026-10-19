//! The Bastion desktop shell (spec slice 3: BMD-15, BMD-27, BMD-28).
//!
//! A tray icon and one window that embeds the primary's web app (`/app`) in a
//! WebView — no chat or memory UI of its own (BMD-27). The tray has:
//! - **Open Bastion** — show/focus the window.
//! - **Stop node** — cut the local node now (BMD-15).
//! - **Quit** — close the shell (the supervised node dies with it).
//!
//! It also follows the primary's presence (see [`presence`]): the tray tooltip
//! says what the sessions are doing, the icon turns amber while something
//! waits for the person, and each pending approval gets an Approve/Deny
//! submenu and, once, a native notification.
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
mod presence;

use std::collections::HashSet;
use std::sync::Arc;

use config::ShellConfig;
use node::NodeSupervisor;
use presence::{ShellEvent, Snapshot};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, Submenu};
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

/// The tray entry for one pending approval: a submenu with Approve (or,
/// when the shown call is cut, "Review in Bastion…", which opens the window
/// where the full call is visible) and Deny.
struct PendingApproval {
    alert_id: String,
    submenu: Submenu,
    approve: MenuId,
    review_only: bool,
    deny: MenuId,
}

fn add_approval(menu: &Menu, alert: &presence::Alert) -> anyhow::Result<PendingApproval> {
    let submenu = Submenu::new(alert.label(), true);
    let review_only = !alert.fully_shown();
    let approve = MenuItem::new(
        if review_only {
            "Review in Bastion…"
        } else {
            "Approve"
        },
        true,
        None,
    );
    let deny = MenuItem::new("Deny", true, None);
    submenu.append(&approve)?;
    submenu.append(&deny)?;
    // Above Open/Stop/Quit, where the eye lands first.
    menu.insert(&submenu, 0)?;
    Ok(PendingApproval {
        alert_id: alert.id.clone(),
        submenu,
        approve: approve.id().clone(),
        review_only,
        deny: deny.id().clone(),
    })
}

/// Bring the tray's approval items in line with the snapshot: drop the ones
/// that left it, add the ones that appeared.
fn sync_approvals(menu: &Menu, pending: &mut Vec<PendingApproval>, snapshot: &Snapshot) {
    let current: HashSet<&str> = presence::approvals(snapshot)
        .map(|a| a.id.as_str())
        .collect();
    pending.retain(|p| {
        let keep = current.contains(p.alert_id.as_str());
        if !keep {
            let _ = menu.remove(&p.submenu);
        }
        keep
    });
    for alert in presence::approvals(snapshot) {
        if pending.iter().any(|p| p.alert_id == alert.id) {
            continue;
        }
        match add_approval(menu, alert) {
            Ok(entry) => pending.push(entry),
            Err(e) => eprintln!("bastion-shell: cannot add the approval item: {e}"),
        }
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

    let event_loop = EventLoopBuilder::<ShellEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
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
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu.clone()))
        .with_tooltip("Bastion")
        .with_icon(presence::tray_icon(false)?)
        .build()?;

    presence::spawn_watcher(config.clone(), proxy);
    let notifications = config.notifications;
    let mut attention = false;
    let mut seen_approvals: HashSet<String> = HashSet::new();
    let mut pending: Vec<PendingApproval> = Vec::new();

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
            Event::UserEvent(ShellEvent::Presence(snapshot)) => {
                let _ = tray.set_tooltip(Some(presence::status_text(&snapshot)));
                let wants_attention = presence::needs_attention(&snapshot);
                if wants_attention != attention {
                    attention = wants_attention;
                    if let Ok(icon) = presence::tray_icon(attention) {
                        let _ = tray.set_icon(Some(icon));
                    }
                }
                let fresh = presence::new_approvals(&mut seen_approvals, &snapshot);
                if notifications {
                    for alert in fresh {
                        // Off the loop: a notification service can be slow.
                        std::thread::spawn(move || presence::notify(&alert));
                    }
                }
                sync_approvals(&menu, &mut pending, &snapshot);
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
            } else if pending
                .iter()
                .any(|p| p.review_only && menu_event.id == p.approve)
            {
                window.set_visible(true);
                window.set_focus();
            } else if let Some((action, entry)) = pending.iter().find_map(|p| {
                if menu_event.id == p.approve {
                    Some(("approve", p))
                } else if menu_event.id == p.deny {
                    Some(("deny", p))
                } else {
                    None
                }
            }) {
                let (config, id) = (config.clone(), entry.alert_id.clone());
                std::thread::spawn(move || presence::post_action(&config, action, &id));
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
