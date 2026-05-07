mod api;
mod api_session;
mod api_terminal;
mod api_transport;
mod api_workspace;
mod app;
mod app_editor;
mod app_state;
mod app_update;
mod composer;
mod conversation;
mod editor;
mod input;
mod model;
mod model_domain;
mod model_event;
mod model_format;
mod model_wire;
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

use crate::{api::detect_base_url, app::App};

#[tokio::main]
async fn main() -> Result<()> {
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

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}
