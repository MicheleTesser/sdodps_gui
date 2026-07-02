use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::can::CanTransport;
use crate::dbc::{Database, SdoBoardDef, Value, ValueKind, ValueStorage};
use crate::sdo::{get_with_response, set_with_ack};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportFile {
    pub version: u32,
    pub entries: Vec<ExportEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportEntry {
    pub board: String,
    pub variable: String,
    pub type_name: String,
    pub var_id: u16,
    pub unit: Option<String>,
    pub value: ExportValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExportValue {
    Unsigned { value: u64 },
    Signed { value: i64 },
    Float { value: f64 },
}

#[derive(Debug, Clone)]
pub struct ImportReport {
    pub applied: usize,
    pub missing: Vec<String>,
    pub type_mismatches: Vec<String>,
}

pub fn export_live(
    database: &Database,
    transport: &CanTransport,
    board_filter: Option<&str>,
    timeout: Duration,
) -> Result<ExportFile> {
    let mut entries = Vec::new();

    for board in filtered_boards(database, board_filter)? {
        for variable in &board.variables {
            let value = get_with_response(transport, board, variable, timeout)?;
            entries.push(ExportEntry {
                board: board.name.clone(),
                variable: variable.name.clone(),
                type_name: storage_tag(variable.storage),
                var_id: variable.id,
                unit: variable.unit.clone(),
                value: value_to_export(value),
            });
        }
    }

    entries.sort_by(|left, right| {
        left.board
            .cmp(&right.board)
            .then(left.variable.cmp(&right.variable))
            .then(left.type_name.cmp(&right.type_name))
    });

    Ok(ExportFile { version: 1, entries })
}

pub fn save_export_file(
    dir: impl AsRef<Path>,
    export: &ExportFile,
    output: Option<&Path>,
    scope: Option<&str>,
) -> Result<PathBuf> {
    let path = match output {
        Some(path) => path.to_path_buf(),
        None => {
            let dir = dir.as_ref();
            fs::create_dir_all(dir)
                .with_context(|| format!("failed to create export dir {}", dir.display()))?;
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context("system clock before unix epoch")?
                .as_secs();
            let scope = scope.unwrap_or("all");
            dir.join(format!("export_{scope}_{stamp}.toml"))
        }
    };

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create export dir {}", parent.display()))?;
    }

    let raw = toml::to_string_pretty(export).context("failed to serialize export file")?;
    fs::write(&path, raw)
        .with_context(|| format!("failed to write export file {}", path.display()))?;
    Ok(path)
}

pub fn apply_export_file(
    path: &Path,
    database: &Database,
    transport: &CanTransport,
    board_filter: Option<&str>,
    timeout: Duration,
) -> Result<ImportReport> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read export file {}", path.display()))?;
    let export: ExportFile = toml::from_str(&raw)
        .with_context(|| format!("failed to parse export file {}", path.display()))?;

    let mut exact = HashMap::new();
    let mut by_name = HashMap::<(String, String), Vec<String>>::new();

    for board in filtered_boards(database, board_filter)? {
        for variable in &board.variables {
            let type_name = storage_tag(variable.storage);
            exact.insert(
                (board.name.clone(), variable.name.clone(), type_name.clone()),
                (board, variable),
            );
            by_name
                .entry((board.name.clone(), variable.name.clone()))
                .or_default()
                .push(type_name);
        }
    }

    let mut report = ImportReport {
        applied: 0,
        missing: Vec::new(),
        type_mismatches: Vec::new(),
    };

    for entry in export.entries {
        if board_filter.is_some() && board_filter != Some(entry.board.as_str()) {
            continue;
        }

        let key = (
            entry.board.clone(),
            entry.variable.clone(),
            entry.type_name.clone(),
        );
        if let Some((board, variable)) = exact.get(&key).copied() {
            let value = export_to_value(entry.value);
            let _ack = set_with_ack(transport, board, variable, value, timeout)?;
            report.applied += 1;
            continue;
        }

        let name_key = (entry.board.clone(), entry.variable.clone());
        if let Some(types) = by_name.get(&name_key) {
            report.type_mismatches.push(format!(
                "{}.{} export={} dbc={}",
                entry.board,
                entry.variable,
                entry.type_name,
                types.join(",")
            ));
        } else {
            report
                .missing
                .push(format!("{}.{} [{}]", entry.board, entry.variable, entry.type_name));
        }
    }

    Ok(report)
}

pub fn storage_tag(storage: ValueStorage) -> String {
    match storage.kind {
        ValueKind::Unsigned => format!("u{}", storage.bits),
        ValueKind::Signed => format!("i{}", storage.bits),
        ValueKind::Float => format!("f{}", storage.bits),
    }
}

pub fn value_to_export(value: Value) -> ExportValue {
    match value {
        Value::Unsigned(value) => ExportValue::Unsigned { value },
        Value::Signed(value) => ExportValue::Signed { value },
        Value::Float(value) => ExportValue::Float { value },
    }
}

pub fn export_to_value(value: ExportValue) -> Value {
    match value {
        ExportValue::Unsigned { value } => Value::Unsigned(value),
        ExportValue::Signed { value } => Value::Signed(value),
        ExportValue::Float { value } => Value::Float(value),
    }
}

pub fn export_from_cached_values(
    database: &Database,
    values: &HashMap<(u32, u16), (Value, std::time::Instant)>,
) -> ExportFile {
    let mut entries = Vec::new();

    for ((board_message_id, var_id), (value, _)) in values {
        let Some(board) = database
            .sdo_boards
            .iter()
            .find(|item| item.message_id == *board_message_id)
        else {
            continue;
        };
        let Some(variable) = board.variables.iter().find(|item| item.id == *var_id) else {
            continue;
        };

        entries.push(ExportEntry {
            board: board.name.clone(),
            variable: variable.name.clone(),
            type_name: storage_tag(variable.storage),
            var_id: variable.id,
            unit: variable.unit.clone(),
            value: value_to_export(value.clone()),
        });
    }

    entries.sort_by(|left, right| {
        left.board
            .cmp(&right.board)
            .then(left.variable.cmp(&right.variable))
            .then(left.type_name.cmp(&right.type_name))
    });

    ExportFile { version: 1, entries }
}

fn filtered_boards<'a>(database: &'a Database, board_filter: Option<&str>) -> Result<Vec<&'a SdoBoardDef>> {
    let mut boards = Vec::new();

    for board in &database.sdo_boards {
        if board_filter.is_none_or(|filter| filter == board.name) {
            boards.push(board);
        }
    }

    if let Some(board_name) = board_filter
        && boards.is_empty()
    {
        anyhow::bail!("scheda SDO '{board_name}' non trovata");
    }

    Ok(boards)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbc::{SdoVariableDef, ValueKind};

    #[test]
    fn storage_tag_uses_kind_and_bits() {
        assert_eq!(
            storage_tag(ValueStorage {
                kind: ValueKind::Unsigned,
                bits: 8,
            }),
            "u8"
        );
        assert_eq!(
            storage_tag(ValueStorage {
                kind: ValueKind::Signed,
                bits: 16,
            }),
            "i16"
        );
        assert_eq!(
            storage_tag(ValueStorage {
                kind: ValueKind::Float,
                bits: 32,
            }),
            "f32"
        );
    }

    #[test]
    fn export_from_cache_is_order_independent() {
        let database = Database {
            path: PathBuf::from("dbc/can2.dbc"),
            nodes: Vec::new(),
            messages: Default::default(),
            sdo_boards: vec![SdoBoardDef {
                name: "PCU".to_string(),
                message_id: 123,
                variables: vec![
                    SdoVariableDef {
                        id: 42,
                        name: "second".to_string(),
                        storage: ValueStorage {
                            kind: ValueKind::Unsigned,
                            bits: 16,
                        },
                        unit: None,
                    },
                    SdoVariableDef {
                        id: 7,
                        name: "first".to_string(),
                        storage: ValueStorage {
                            kind: ValueKind::Float,
                            bits: 32,
                        },
                        unit: None,
                    },
                ],
            }],
        };

        let mut cached = HashMap::new();
        cached.insert(
            ("PCU".to_string(), 42),
            (Value::Unsigned(99), std::time::Instant::now()),
        );
        cached.insert(
            ("PCU".to_string(), 7),
            (Value::Float(1.5), std::time::Instant::now()),
        );

        let export = export_from_cached_values(&database, &cached);
        let keys = export
            .entries
            .into_iter()
            .map(|entry| (entry.board, entry.variable, entry.type_name))
            .collect::<Vec<_>>();

        assert_eq!(
            keys,
            vec![
                ("PCU".to_string(), "first".to_string(), "f32".to_string()),
                ("PCU".to_string(), "second".to_string(), "u16".to_string()),
            ]
        );
    }
}
