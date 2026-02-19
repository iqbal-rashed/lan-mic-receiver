#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod core;
mod icon;

use single_instance::SingleInstance;
use std::sync::mpsc::channel;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIconBuilder,
};

const APP_INSTANCE_NAME: &str = "lan-mic-receiver-single-instance";

#[derive(Debug, Clone)]
pub enum TrayMessage {
    Show,
    Hide,
    Quit,
}

fn main() -> iced::Result {
    // Install default crypto provider to avoid panic in axum-server/rustls
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Ensure only one instance of the app is running
    let instance = SingleInstance::new(APP_INSTANCE_NAME).unwrap();
    if !instance.is_single() {
        eprintln!("LAN Mic Receiver is already running.");
        std::process::exit(0);
    }

    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    // Create channel for tray communication
    let (tx, rx) = channel::<TrayMessage>();

    // Create tray menu
    let tray_menu = Menu::new();
    let show_item = MenuItem::new("Show", true, None);
    let hide_item = MenuItem::new("Hide", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    tray_menu.append(&show_item).unwrap();
    tray_menu.append(&hide_item).unwrap();
    tray_menu.append(&PredefinedMenuItem::separator()).unwrap();
    tray_menu.append(&quit_item).unwrap();

    // Create tray icon
    let icon_data = icon::create_icon(32);
    let tray_icon_img =
        tray_icon::Icon::from_rgba(icon_data, 32, 32).expect("Failed to create tray icon");

    // Store IDs for menu items
    let show_id = show_item.id().clone();
    let hide_id = hide_item.id().clone();
    let quit_id = quit_item.id().clone();

    // Build tray icon
    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("LAN Mic Receiver")
        .with_icon(tray_icon_img)
        .build()
        .unwrap();

    // Spawn tray event handler thread
    let tx_clone = tx.clone();
    std::thread::spawn(move || {
        let menu_channel = MenuEvent::receiver();
        loop {
            if let Ok(event) = menu_channel.recv() {
                if event.id == show_id {
                    let _ = tx_clone.send(TrayMessage::Show);
                } else if event.id == hide_id {
                    let _ = tx_clone.send(TrayMessage::Hide);
                } else if event.id == quit_id {
                    let _ = tx_clone.send(TrayMessage::Quit);
                    break;
                }
            }
        }
    });

    let shared = core::SharedStatus::default();
    let controller = core::spawn_runtime(shared.clone());

    app::launch_app(controller, shared, rx)
}
