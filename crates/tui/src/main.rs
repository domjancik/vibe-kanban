mod api;
mod app;
mod composer;
mod conversation;
mod conversation_state;
mod editor;
mod input;
mod model;
mod ui;
mod workspace;

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::DefaultTerminal;
use tracing_subscriber::EnvFilter;

use crate::{
    api::{detect_base_url, transport::log_tui},
    app::App,
};

#[tokio::main]
async fn main() -> Result<()> {
    install_panic_logger();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,tui=info")),
        )
        .init();

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = DefaultTerminal::new(backend)?;

    let base_url = detect_base_url();
    let api = api::Api::new(base_url)?;
    let app = App::new(api);
    let result = app.run(&mut terminal).await;
    if let Err(error) = &result {
        log_tui(format!("app.run failed: {error:#}"));
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn install_panic_logger() {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let location = panic_info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "unknown location".to_string());
        let payload = if let Some(message) = panic_info.payload().downcast_ref::<&str>() {
            (*message).to_string()
        } else if let Some(message) = panic_info.payload().downcast_ref::<String>() {
            message.clone()
        } else {
            "non-string panic payload".to_string()
        };
        let backtrace = std::backtrace::Backtrace::force_capture();
        log_tui(format!(
            "panic at {location}: {payload}\nbacktrace:\n{backtrace}"
        ));
        previous_hook(panic_info);
    }));
}
