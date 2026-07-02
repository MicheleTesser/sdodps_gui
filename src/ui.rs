use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, Wrap};

use crate::app::{App, Pane};

pub fn render(frame: &mut Frame, app: &App) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(10),
            Constraint::Length(10),
        ])
        .split(frame.area());

    render_header(frame, layout[0], app);

    let middle = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(28), Constraint::Min(40)])
        .split(layout[1]);

    render_boards(frame, middle[0], app);
    render_variables(frame, middle[1], app);
    render_log(frame, layout[2], app);

    if let Some(prompt) = app.current_prompt() {
        render_prompt(frame, centered_rect(70, 20, frame.area()), &prompt);
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let filter = app
        .board_filter
        .and_then(|message_id| {
            app.database
                .sdo_boards
                .iter()
                .find(|board| board.message_id == message_id)
                .map(|board| app.board_label(board))
        })
        .unwrap_or_else(|| "*".to_string());
    let dbcc = app
        .dbcc
        .generated_dir
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "disabled".to_string());
    let dbcc_exec = app
        .dbcc
        .executable
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "-".to_string());
    let bindings = app
        .dbcc
        .bindings_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "-".to_string());
    let text = vec![
        Line::from(vec![
            Span::styled("DBC ", Style::default().fg(Color::Cyan)),
            Span::raw(app.database.path.display().to_string()),
            Span::raw("  "),
            Span::styled("CFG ", Style::default().fg(Color::LightBlue)),
            Span::raw(app.runtime.config_path.display().to_string()),
            Span::raw("  "),
            Span::styled("CAN ", Style::default().fg(Color::Yellow)),
            Span::raw(app.runtime.socketcan.clone()),
        ]),
        Line::from(vec![
            Span::styled("Nodes ", Style::default().fg(Color::Green)),
            Span::raw(app.database.nodes.len().to_string()),
            Span::raw("  "),
            Span::styled("Msgs ", Style::default().fg(Color::Green)),
            Span::raw(app.database.messages.len().to_string()),
            Span::raw("  "),
            Span::styled("Filtro scheda ", Style::default().fg(Color::Green)),
            Span::raw(filter),
            Span::raw("  "),
            Span::styled("Ricerca ", Style::default().fg(Color::Magenta)),
            Span::raw(if app.search_filter.is_empty() {
                "-".to_string()
            } else {
                app.search_filter.clone()
            }),
            Span::raw("  "),
            Span::styled("dbcc ", Style::default().fg(Color::LightCyan)),
            Span::raw(dbcc),
            Span::raw("  "),
            Span::styled("Stato ", Style::default().fg(Color::Blue)),
            Span::raw(app.last_status.clone()),
        ]),
        Line::from(vec![
            Span::styled("dbcc-bin ", Style::default().fg(Color::LightBlue)),
            Span::raw(dbcc_exec),
            Span::raw("  "),
            Span::styled("bindings ", Style::default().fg(Color::LightBlue)),
            Span::raw(bindings),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Sessione")),
        area,
    );
}

fn render_boards(frame: &mut Frame, area: Rect, app: &App) {
    let labels = app.board_labels();
    let (offset, len) = app.board_window(area.height.saturating_sub(2) as usize);
    let items = labels
        .into_iter()
        .enumerate()
        .skip(offset)
        .take(len)
        .map(|(index, board)| {
            let style = if index == app.board_index && app.pane == Pane::Boards {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(board)).style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Schede SDO [Tab, f, a]"),
        ),
        area,
    );
}

fn render_variables(frame: &mut Frame, area: Rect, app: &App) {
    let rows = app.visible_rows();
    let (offset, len) = app.variable_window(area.height.saturating_sub(3) as usize);
    let table_rows = rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(len)
        .map(|(index, row)| {
            let style = if index == app.variable_index && app.pane == Pane::Variables {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightYellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Row::new([
                Cell::from(row.board.clone()),
                Cell::from(row.var_id.to_string()),
                Cell::from(row.name.clone()),
                Cell::from(format!("{:?}{}", row.storage.kind, row.storage.bits)),
                Cell::from(
                    row.value
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "-".to_string()),
                ),
                Cell::from(
                    row.updated_at
                        .map(|instant| format!("{:.1}s", instant.elapsed().as_secs_f32()))
                        .unwrap_or_else(|| "-".to_string()),
                ),
                Cell::from(row.unit.clone().unwrap_or_else(|| "-".to_string())),
            ])
            .style(style)
        })
        .collect::<Vec<_>>();

    let widths = [
        Constraint::Length(10),
        Constraint::Length(6),
        Constraint::Min(20),
        Constraint::Length(10),
        Constraint::Length(16),
        Constraint::Length(8),
        Constraint::Length(8),
    ];

    frame.render_widget(
        Table::new(table_rows, widths)
            .header(
                Row::new(["Board", "Id", "Variable", "Type", "Value", "Age", "Unit"]).style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            )
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Variabili [g, G, e, /, s, r]"),
            )
            .column_spacing(1),
        area,
    );
}

fn render_log(frame: &mut Frame, area: Rect, app: &App) {
    let lines = app
        .log_lines()
        .into_iter()
        .map(Line::from)
        .collect::<Vec<_>>();

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Ultima Operazione / Log [q per uscire]"),
        ),
        area,
    );
}

fn render_prompt(frame: &mut Frame, area: Rect, prompt: &str) {
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(prompt)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title("Input")),
        area,
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup[1])[1]
}
