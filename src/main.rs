mod backup;
mod config;
mod icon;
mod instance;
mod integrity;
mod login;
mod macos;
mod paths;
mod retention;
mod rsync;
mod schedule;
mod scheduler;
mod state;
mod stats;
mod tray;
mod ui;

use std::sync::Arc;

use eframe::egui;
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

use crate::config::Config;
use crate::scheduler::Engine;
use crate::state::State;

fn main() -> eframe::Result<()> {
    let background = std::env::args().any(|a| a == login::BACKGROUND_ARG);
    let Some(_instance) = instance::acquire(background) else {
        eprintln!("MacSync is already running.");
        return Ok(());
    };

    let (config, notice) = Config::load();
    let start_visible = !background || config.pairs.is_empty();
    let engine = Engine::new(config, State::load(), notice);
    engine.start();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MacSync")
            .with_app_id("com.macsync.app")
            .with_inner_size([980.0, 720.0])
            .with_min_inner_size([760.0, 520.0])
            .with_visible(start_visible),
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_activation_policy(ActivationPolicy::Accessory);
        })),
        ..Default::default()
    };

    eframe::run_native(
        "MacSync",
        options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            let weak = Arc::downgrade(&engine);
            let repaint_ctx = ctx.clone();
            engine.set_on_change(move || {
                repaint_ctx.request_repaint();
                tray::request_refresh(weak.clone());
            });
            tray::install(&engine, &ctx)?;
            let show_ctx = ctx.clone();
            instance::set_on_show(move || macos::request_show(&show_ctx));
            if start_visible {
                macos::activate_app();
            }
            Ok(Box::new(ui::MacSyncApp::new(engine, !start_visible)))
        }),
    )
}
