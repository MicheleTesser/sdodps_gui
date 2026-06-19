use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::Instant;

use anyhow::{Result, anyhow};
use crossterm::event::KeyCode;

use crate::can::{CanFrame, CanTransport};
use crate::config::RuntimeConfig;
use crate::dbc::{Database, SdoBoardDef, Value, ValueKind, ValueStorage};
use crate::dbcc::DbccRuntime;
use crate::exports::{export_from_cached_values, save_export_file};

const OPCODE_GET_REQ: u64 = 1;
const OPCODE_SET_REQ: u64 = 2;
const OPCODE_RES: u64 = 128;
const OPCODE_ERR_OUT_OF_RANGE: u64 = 253;
const OPCODE_ERR_WRITE_RO: u64 = 254;
const OPCODE_ERR: u64 = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Boards,
    Variables,
    Log,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Board,
    Variable,
    VarId,
    Type,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone)]
pub struct VariableRow {
    pub board: String,
    pub var_id: u16,
    pub name: String,
    pub storage: ValueStorage,
    pub unit: Option<String>,
    pub value: Option<Value>,
    pub updated_at: Option<Instant>,
}

#[derive(Debug, Clone)]
pub struct OperationLogEntry {
    pub timestamp: Instant,
    pub title: String,
    pub detail: String,
}

pub struct App {
    pub runtime: RuntimeConfig,
    pub database: Database,
    pub dbcc: DbccRuntime,
    pub pane: Pane,
    pub board_index: usize,
    pub variable_index: usize,
    pub sort_field: SortField,
    pub sort_direction: SortDirection,
    pub board_filter: Option<String>,
    pub search_filter: String,
    pub logs: Vec<OperationLogEntry>,
    pub last_status: String,
    pub should_quit: bool,
    pub transport: Option<CanTransport>,
    pub transport_error: Option<String>,
    pub sdo_values: HashMap<(String, u16), (Value, Instant)>,
    pub input_mode: Option<InputMode>,
}

#[derive(Debug, Clone)]
pub enum InputMode {
    Search,
    SetValue {
        board: String,
        var_id: u16,
        name: String,
        storage: ValueStorage,
        buffer: String,
    },
}

impl App {
    pub fn new(
        runtime: RuntimeConfig,
        database: Database,
        transport: Result<CanTransport>,
        dbcc: DbccRuntime,
    ) -> Self {
        let (transport, transport_error) = match transport {
            Ok(transport) => (Some(transport), None),
            Err(error) => (None, Some(error.to_string())),
        };

        let mut app = Self {
            runtime,
            database,
            dbcc,
            pane: Pane::Boards,
            board_index: 0,
            variable_index: 0,
            sort_field: SortField::Board,
            sort_direction: SortDirection::Asc,
            board_filter: None,
            search_filter: String::new(),
            logs: Vec::new(),
            last_status: String::new(),
            should_quit: false,
            transport,
            transport_error,
            sdo_values: HashMap::new(),
            input_mode: None,
        };

        app.push_status(
            "startup",
            format!(
                "dbc={} can={} dbcc={}",
                app.runtime.dbc_path.display(),
                app.runtime.socketcan,
                app.dbcc.status
            ),
        );
        if let Some(error) = &app.transport_error {
            app.push_status("socketcan", error.clone());
        }
        app
    }

    pub fn tick(&mut self) -> Result<()> {
        self.poll_can()
    }

    pub fn handle_key(&mut self, key: KeyCode) -> Result<()> {
        if matches!(self.input_mode, Some(InputMode::Search)) {
            return self.handle_search_input(key);
        }
        if matches!(self.input_mode, Some(InputMode::SetValue { .. })) {
            return self.handle_set_input(key);
        }

        match key {
            KeyCode::Tab => self.focus_next_pane(),
            KeyCode::BackTab => self.focus_prev_pane(),
            KeyCode::Down => self.select_next(),
            KeyCode::Up => self.select_prev(),
            KeyCode::Char('a') => {
                self.board_filter = None;
                self.variable_index = 0;
                self.push_status("filter", "visualizzo tutte le schede".to_string());
            }
            KeyCode::Char('f') => {
                if let Some(board) = self.selected_board_name() {
                    self.board_filter = Some(board.clone());
                    self.variable_index = 0;
                    self.push_status("filter", format!("filtro sulla scheda {board}"));
                }
            }
            KeyCode::Char('/') => self.input_mode = Some(InputMode::Search),
            KeyCode::Char('s') => self.cycle_sort_field(),
            KeyCode::Char('r') => self.toggle_sort_direction(),
            KeyCode::Char('g') => self.get_selected()?,
            KeyCode::Char('G') => self.get_all_for_current_scope()?,
            KeyCode::Enter | KeyCode::Char('e') => self.begin_set_selected()?,
            KeyCode::Char('x') => {
                let path = self.export_current_values()?;
                self.push_status("export", format!("salvato {}", path.display()));
            }
            KeyCode::Esc => {
                self.board_filter = None;
                self.search_filter.clear();
                self.push_status("filter", "filtri azzerati".to_string());
            }
            _ => {}
        }
        Ok(())
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn board_labels(&self) -> Vec<String> {
        self.database
            .sdo_boards
            .iter()
            .map(|board| format!("{} [SDO]", board.name))
            .collect()
    }

    pub fn visible_rows(&self) -> Vec<VariableRow> {
        let mut rows = Vec::new();

        for board in &self.database.sdo_boards {
            if !self.matches_board_filter(&board.name) {
                continue;
            }
            for variable in &board.variables {
                if !self.matches_search_filter(&variable.name) {
                    continue;
                }
                let value = self.lookup_sdo_value(board, variable.id);
                rows.push(VariableRow {
                    board: board.name.clone(),
                    var_id: variable.id,
                    name: variable.name.clone(),
                    storage: variable.storage,
                    unit: variable.unit.clone(),
                    value: value.as_ref().map(|entry| entry.0.clone()),
                    updated_at: value.as_ref().map(|entry| entry.1),
                });
            }
        }

        rows.sort_by(|left, right| self.compare_rows(left, right));
        rows
    }

    pub fn selected_row(&self) -> Option<VariableRow> {
        self.visible_rows().get(self.variable_index).cloned()
    }

    pub fn current_prompt(&self) -> Option<String> {
        match &self.input_mode {
            Some(InputMode::Search) => Some(format!("Filtro: {}", self.search_filter)),
            Some(InputMode::SetValue {
                board,
                name,
                buffer,
                storage,
                ..
            }) => Some(format!(
                "Set {board}.{name} [{}{}]: {buffer}",
                describe_value_kind(storage.kind),
                storage.bits
            )),
            None => None,
        }
    }

    pub fn log_lines(&self) -> Vec<String> {
        self.logs
            .iter()
            .rev()
            .take(12)
            .map(|entry| {
                format!(
                    "[{:.1}s] {}: {}",
                    entry.timestamp.elapsed().as_secs_f32(),
                    entry.title,
                    entry.detail
                )
            })
            .collect()
    }

    fn poll_can(&mut self) -> Result<()> {
        loop {
            let frame = {
                let Some(transport) = &self.transport else {
                    return Ok(());
                };
                transport.read_frame()?
            };
            match frame {
                Some(frame) => self.handle_frame(frame)?,
                None => break,
            }
        }
        Ok(())
    }

    fn handle_frame(&mut self, frame: CanFrame) -> Result<()> {
        if let Some(board) = self
            .database
            .sdo_boards
            .iter()
            .find(|board| board.message_id == frame.id)
            .cloned()
        {
            return self.handle_sdo_frame(&board, &frame);
        }

        Ok(())
    }

    fn handle_sdo_frame(&mut self, board: &SdoBoardDef, frame: &CanFrame) -> Result<()> {
        let data = &frame.data;
        let opcode = extract_bits(data, 0, 8);
        let var_id = extract_bits(data, 8, 10) as u16;

        match opcode {
            OPCODE_RES => {
                let Some(variable) = board
                    .variables
                    .iter()
                    .find(|variable| variable.id == var_id)
                else {
                    self.push_status(
                        "sdo",
                        format!("{} risposta per var_id sconosciuto {}", board.name, var_id),
                    );
                    return Ok(());
                };
                let raw = extract_bits(data, 24, variable.storage.bits);
                let value = Value::decode_raw(variable.storage, raw);
                self.store_sdo_value(&board.name, var_id, value.clone());
                self.push_status(
                    "sdo",
                    format!("{} {} => {}", board.name, variable.name, value),
                );
            }
            OPCODE_ERR_OUT_OF_RANGE => {
                self.push_status(
                    "sdo",
                    format!("{} var_id {} fuori range", board.name, var_id),
                );
            }
            OPCODE_ERR_WRITE_RO => {
                self.push_status("sdo", format!("{} var_id {} readonly", board.name, var_id));
            }
            OPCODE_ERR => {
                self.push_status(
                    "sdo",
                    format!("{} errore generico su var_id {}", board.name, var_id),
                );
            }
            other => {
                self.push_status("sdo", format!("{} opcode {} inatteso", board.name, other));
            }
        }
        Ok(())
    }

    fn get_selected(&mut self) -> Result<()> {
        let Some(row) = self.selected_row() else {
            return Ok(());
        };

        self.send_sdo_get(&row.board, row.var_id)
    }

    fn get_all_for_current_scope(&mut self) -> Result<()> {
        let rows = self.visible_rows();
        if rows.is_empty() {
            return Ok(());
        }

        self.push_status("get_all", format!("invio {} richieste", rows.len()));
        for row in rows {
            self.send_sdo_get(&row.board, row.var_id)?;
        }

        Ok(())
    }

    fn begin_set_selected(&mut self) -> Result<()> {
        let Some(row) = self.selected_row() else {
            return Ok(());
        };
        self.input_mode = Some(InputMode::SetValue {
            board: row.board,
            var_id: row.var_id,
            name: row.name,
            storage: row.storage,
            buffer: String::new(),
        });
        Ok(())
    }

    fn handle_search_input(&mut self, key: KeyCode) -> Result<()> {
        match key {
            KeyCode::Esc | KeyCode::Enter => self.input_mode = None,
            KeyCode::Backspace => {
                self.search_filter.pop();
            }
            KeyCode::Char(ch) => self.search_filter.push(ch),
            _ => {}
        }
        self.variable_index = 0;
        Ok(())
    }

    fn handle_set_input(&mut self, key: KeyCode) -> Result<()> {
        match key {
            KeyCode::Esc => self.input_mode = None,
            KeyCode::Backspace => {
                if let Some(InputMode::SetValue { buffer, .. }) = &mut self.input_mode {
                    buffer.pop();
                }
            }
            KeyCode::Enter => {
                let Some(InputMode::SetValue {
                    board,
                    var_id,
                    storage,
                    buffer,
                    ..
                }) = self.input_mode.take()
                else {
                    return Ok(());
                };

                let value = Value::parse(&buffer, storage)?;
                self.send_sdo_set(&board, var_id, value)?;
            }
            KeyCode::Char(ch) => {
                if let Some(InputMode::SetValue { buffer, .. }) = &mut self.input_mode {
                    buffer.push(ch);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn send_sdo_get(&mut self, board_name: &str, var_id: u16) -> Result<()> {
        let Some(board) = self
            .database
            .sdo_boards
            .iter()
            .find(|item| item.name == board_name)
        else {
            return Err(anyhow!("scheda SDO '{board_name}' non trovata"));
        };
        let Some(transport) = &self.transport else {
            return Err(anyhow!("socketcan non disponibile"));
        };

        let mut payload = [0u8; 7];
        insert_bits(&mut payload, 0, 8, OPCODE_GET_REQ);
        insert_bits(&mut payload, 8, 10, var_id as u64);
        transport.write_frame(board.message_id, &payload)?;
        self.push_status("get", format!("{} var_id {}", board.name, var_id));
        Ok(())
    }

    fn send_sdo_set(&mut self, board_name: &str, var_id: u16, value: Value) -> Result<()> {
        let Some(board) = self
            .database
            .sdo_boards
            .iter()
            .find(|item| item.name == board_name)
        else {
            return Err(anyhow!("scheda SDO '{board_name}' non trovata"));
        };
        let Some(variable) = board.variables.iter().find(|item| item.id == var_id) else {
            return Err(anyhow!("variabile {var_id} non trovata su {board_name}"));
        };
        let Some(transport) = &self.transport else {
            return Err(anyhow!("socketcan non disponibile"));
        };

        let raw = value.clone().encode_raw(variable.storage)?;
        let mut payload = [0u8; 7];
        insert_bits(&mut payload, 0, 8, OPCODE_SET_REQ);
        insert_bits(&mut payload, 8, 10, var_id as u64);
        insert_bits(&mut payload, 24, variable.storage.bits, raw);
        transport.write_frame(board.message_id, &payload)?;
        self.push_status(
            "set",
            format!("{} {} <= {}", board.name, variable.name, value),
        );
        Ok(())
    }

    fn focus_next_pane(&mut self) {
        self.pane = match self.pane {
            Pane::Boards => Pane::Variables,
            Pane::Variables => Pane::Log,
            Pane::Log => Pane::Boards,
        };
    }

    fn focus_prev_pane(&mut self) {
        self.pane = match self.pane {
            Pane::Boards => Pane::Log,
            Pane::Variables => Pane::Boards,
            Pane::Log => Pane::Variables,
        };
    }

    fn select_next(&mut self) {
        match self.pane {
            Pane::Boards => {
                let len = self.board_labels().len();
                if len > 0 {
                    self.board_index = (self.board_index + 1) % len;
                }
            }
            Pane::Variables => {
                let len = self.visible_rows().len();
                if len > 0 {
                    self.variable_index = (self.variable_index + 1) % len;
                }
            }
            Pane::Log => {}
        }
    }

    fn select_prev(&mut self) {
        match self.pane {
            Pane::Boards => {
                let len = self.board_labels().len();
                if len > 0 {
                    self.board_index = (self.board_index + len - 1) % len;
                }
            }
            Pane::Variables => {
                let len = self.visible_rows().len();
                if len > 0 {
                    self.variable_index = (self.variable_index + len - 1) % len;
                }
            }
            Pane::Log => {}
        }
    }

    fn selected_board_name(&self) -> Option<String> {
        let labels = self.board_labels();
        labels
            .get(self.board_index)
            .map(|label| label.split_whitespace().next().unwrap_or(label).to_string())
    }

    fn cycle_sort_field(&mut self) {
        self.sort_field = match self.sort_field {
            SortField::Board => SortField::Variable,
            SortField::Variable => SortField::VarId,
            SortField::VarId => SortField::Type,
            SortField::Type => SortField::Board,
        };
        self.push_status("sort", format!("campo {:?}", self.sort_field));
    }

    fn toggle_sort_direction(&mut self) {
        self.sort_direction = match self.sort_direction {
            SortDirection::Asc => SortDirection::Desc,
            SortDirection::Desc => SortDirection::Asc,
        };
        self.push_status("sort", format!("direzione {:?}", self.sort_direction));
    }

    fn matches_board_filter(&self, board_name: &str) -> bool {
        self.board_filter
            .as_ref()
            .map(|filter| filter == board_name)
            .unwrap_or(true)
    }

    fn matches_search_filter(&self, variable_name: &str) -> bool {
        self.search_filter.is_empty()
            || variable_name
                .to_ascii_lowercase()
                .contains(&self.search_filter.to_ascii_lowercase())
    }

    fn compare_rows(&self, left: &VariableRow, right: &VariableRow) -> Ordering {
        let ordering = match self.sort_field {
            SortField::Board => left
                .board
                .cmp(&right.board)
                .then(left.var_id.cmp(&right.var_id)),
            SortField::Variable => left
                .name
                .cmp(&right.name)
                .then(left.board.cmp(&right.board)),
            SortField::VarId => left
                .var_id
                .cmp(&right.var_id)
                .then(left.board.cmp(&right.board)),
            SortField::Type => describe_storage(left.storage)
                .cmp(&describe_storage(right.storage))
                .then(left.board.cmp(&right.board)),
        };
        match self.sort_direction {
            SortDirection::Asc => ordering,
            SortDirection::Desc => ordering.reverse(),
        }
    }

    fn push_status(&mut self, title: impl Into<String>, detail: String) {
        self.last_status = detail.clone();
        self.logs.push(OperationLogEntry {
            timestamp: Instant::now(),
            title: title.into(),
            detail,
        });
        if self.logs.len() > 128 {
            let drain = self.logs.len() - 128;
            self.logs.drain(0..drain);
        }
    }

    fn lookup_sdo_value(&self, board: &SdoBoardDef, var_id: u16) -> Option<(Value, Instant)> {
        self.sdo_values
            .get(&(board.name.clone(), var_id))
            .map(|entry| (entry.0.clone(), entry.1))
    }

    fn store_sdo_value(&mut self, board_name: &str, var_id: u16, value: Value) {
        self.sdo_values
            .insert((board_name.to_string(), var_id), (value, Instant::now()));
    }

    fn export_current_values(&self) -> Result<std::path::PathBuf> {
        let export = export_from_cached_values(&self.database, &self.sdo_values);
        save_export_file("exports", &export, None, None)
    }
}

fn describe_storage(storage: ValueStorage) -> String {
    format!("{}{}", describe_value_kind(storage.kind), storage.bits)
}

fn describe_value_kind(kind: ValueKind) -> &'static str {
    match kind {
        ValueKind::Unsigned => "u",
        ValueKind::Signed => "i",
        ValueKind::Float => "f",
    }
}

fn insert_bits(buffer: &mut [u8], start_bit: u16, bit_len: u16, value: u64) {
    for offset in 0..bit_len {
        let bit = ((value >> offset) & 1) as u8;
        let absolute = start_bit + offset;
        let byte_index = (absolute / 8) as usize;
        let bit_index = (absolute % 8) as u8;
        if bit == 1 {
            buffer[byte_index] |= 1u8 << bit_index;
        } else {
            buffer[byte_index] &= !(1u8 << bit_index);
        }
    }
}

fn extract_bits(buffer: &[u8], start_bit: u16, bit_len: u16) -> u64 {
    let mut value = 0u64;
    for offset in 0..bit_len {
        let absolute = start_bit + offset;
        let byte_index = (absolute / 8) as usize;
        let bit_index = (absolute % 8) as u8;
        let bit = (buffer[byte_index] >> bit_index) & 1;
        value |= u64::from(bit) << offset;
    }
    value
}
