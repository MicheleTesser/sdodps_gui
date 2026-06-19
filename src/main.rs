mod app;
mod can;
mod config;
mod dbc;
mod dbcc;
mod exports;
mod sdo;
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
use crate::config::{Command, RuntimeConfig};
use crate::dbc::Database;
use crate::dbcc::DbccRuntime;
use crate::exports::{apply_export_file, export_live, save_export_file, storage_tag};
use crate::sdo::{get_with_response, parse_cli_value, variable_by_name};

fn main() -> Result<()> {
    let (runtime, args) = RuntimeConfig::load_with_args()?;
    let database = Database::load(&runtime.dbc_path)?;

    if let Some(command) = args.command.as_ref() {
        run_command(command, &runtime, &database)?;
        return Ok(());
    }

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

fn run_command(
    command: &Command,
    runtime: &RuntimeConfig,
    database: &Database,
) -> Result<()> {
    match command {
        Command::List(command) => {
            if let Some(board_name) = command.board.as_deref() {
                let board = crate::sdo::board_by_name(database, board_name)?;
                for variable in &board.variables {
                    println!(
                        "{}\t{}\t{}\t{}",
                        board.name,
                        variable.name,
                        storage_tag(variable.storage),
                        variable.id
                    );
                }
            } else {
                for board in &database.sdo_boards {
                    println!("{}\t{}", board.name, board.variables.len());
                }
            }
        }
        Command::Export(command) => {
            let transport = open_transport(&runtime.socketcan)?;
            let export = export_live(
                database,
                &transport,
                command.board.as_deref(),
                runtime.timeout,
            )?;
            let scope = command.board.as_deref();
            let path = save_export_file("exports", &export, command.output.as_deref(), scope)?;
            println!(
                "export completato: {} entry salvate in {}",
                export.entries.len(),
                path.display()
            );
        }
        Command::Restore(command) => {
            let transport = open_transport(&runtime.socketcan)?;
            let report = apply_export_file(
                &command.input,
                database,
                &transport,
                command.board.as_deref(),
                runtime.timeout,
            )?;
            println!(
                "restore completato: applicati {}, mancanti {}, tipo diverso {}",
                report.applied,
                report.missing.len(),
                report.type_mismatches.len()
            );
            for item in &report.missing {
                println!("missing: {item}");
            }
            for item in &report.type_mismatches {
                println!("type-mismatch: {item}");
            }
        }
        Command::Get(command) => {
            let transport = open_transport(&runtime.socketcan)?;
            let target = variable_by_name(database, &command.board, &command.variable)?;
            let value = get_with_response(
                &transport,
                target.board,
                target.variable,
                runtime.timeout,
            )?;
            println!(
                "{}.{} [{}] = {}",
                target.board.name,
                target.variable.name,
                storage_tag(target.variable.storage),
                value
            );
        }
        Command::Set(command) => {
            let transport = open_transport(&runtime.socketcan)?;
            let target = variable_by_name(database, &command.board, &command.variable)?;
            let value = parse_cli_value(&command.value, target.variable.storage)?;
            let ack = crate::sdo::set_with_ack(
                &transport,
                target.board,
                target.variable,
                value,
                runtime.timeout,
            )?;
            println!(
                "ack {}.{} [{}] = {}",
                target.board.name,
                target.variable.name,
                storage_tag(target.variable.storage),
                ack
            );
        }
    }

    Ok(())
}

fn open_transport(interface: &str) -> Result<CanTransport> {
    CanTransport::open(interface).map_err(|error| {
        anyhow::anyhow!("cannot open socketcan interface '{}': {error}", interface)
    })
}
