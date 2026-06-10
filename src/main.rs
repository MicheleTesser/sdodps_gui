mod app;
mod can;
mod config;
mod dbc;
mod dbcc;
mod ui;

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::App;
use crate::can::CanTransport;
use crate::config::RuntimeConfig;
use crate::dbc::Database;
use crate::dbcc::DbccRuntime;

fn main() -> Result<()> {
    let runtime = RuntimeConfig::load()?;
    let database = Database::load(&runtime.dbc_path)?;
    let dbcc = DbccRuntime::prepare(&runtime)?;
    let transport = CanTransport::open(&runtime.socketcan).map_err(|error| {
        anyhow::anyhow!(
            "cannot open socketcan interface '{}': {error}",
            runtime.socketcan
        )
    });

    let mut app = App::new(runtime, database, transport, dbcc);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        app.tick()?;
        terminal.draw(|frame| ui::render(frame, app))?;

        if app.should_quit() {
            break;
        }

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') => app.quit(),
                other => app.handle_key(other)?,
            }
        }
    }

    Ok(())
}
