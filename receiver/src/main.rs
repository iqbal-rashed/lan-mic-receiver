mod app;
mod core;
mod audio;

fn main() -> iced::Result {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    let shared = core::SharedStatus::default();
    let controller = core::spawn_runtime(shared.clone());

    app::launch_app(controller, shared)
}
