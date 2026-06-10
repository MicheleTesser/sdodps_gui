use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use regex::Regex;

#[derive(Debug, Clone)]
pub struct Database {
    pub path: PathBuf,
    pub nodes: Vec<String>,
    pub messages: BTreeMap<u32, MessageDef>,
    pub sdo_boards: Vec<SdoBoardDef>,
}

#[derive(Debug, Clone)]
pub struct MessageDef {
    pub id: u32,
    pub name: String,
    pub sender: String,
    pub signals: Vec<SignalDef>,
    pub value_tables: HashMap<String, BTreeMap<i64, String>>,
}

#[derive(Debug, Clone)]
pub struct SignalDef {
    pub name: String,
    pub start_bit: u16,
    pub bit_len: u16,
    pub byte_order: ByteOrder,
    pub signed: bool,
    pub factor: f64,
    pub offset: f64,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub unit: Option<String>,
    pub receivers: Vec<String>,
    pub mux: MuxRole,
    pub value_storage: ValueStorage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    LittleEndian,
    BigEndian,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuxRole {
    None,
    Multiplexor,
    Multiplexed(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Unsigned,
    Signed,
    Float,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueStorage {
    pub kind: ValueKind,
    pub bits: u16,
}

#[derive(Debug, Clone)]
pub struct SdoBoardDef {
    pub name: String,
    pub message_id: u32,
    pub variables: Vec<SdoVariableDef>,
}

#[derive(Debug, Clone)]
pub struct SdoVariableDef {
    pub id: u16,
    pub name: String,
    pub storage: ValueStorage,
    pub unit: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Value {
    Unsigned(u64),
    Signed(i64),
    Float(f64),
}

impl Database {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read dbc file {}", path.display()))?;
        parse_database(path, &raw)
    }
}

impl ValueStorage {
    fn from_signal(signal: &ParsedSignal, value_type: Option<i32>) -> Self {
        match value_type {
            Some(1) => Self {
                kind: ValueKind::Float,
                bits: 32,
            },
            Some(2) => Self {
                kind: ValueKind::Float,
                bits: 64,
            },
            _ if signal.signed => Self {
                kind: ValueKind::Signed,
                bits: normalize_storage_bits(signal.bit_len),
            },
            _ => Self {
                kind: ValueKind::Unsigned,
                bits: normalize_storage_bits(signal.bit_len),
            },
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsigned(value) => write!(formatter, "{value}"),
            Self::Signed(value) => write!(formatter, "{value}"),
            Self::Float(value) => write!(formatter, "{value:.4}"),
        }
    }
}

impl Value {
    pub fn parse(input: &str, storage: ValueStorage) -> Result<Self> {
        Ok(match storage.kind {
            ValueKind::Unsigned => Self::Unsigned(
                input
                    .trim()
                    .parse::<u64>()
                    .with_context(|| format!("invalid unsigned value '{input}'"))?,
            ),
            ValueKind::Signed => Self::Signed(
                input
                    .trim()
                    .parse::<i64>()
                    .with_context(|| format!("invalid signed value '{input}'"))?,
            ),
            ValueKind::Float => Self::Float(
                input
                    .trim()
                    .parse::<f64>()
                    .with_context(|| format!("invalid float value '{input}'"))?,
            ),
        })
    }

    pub fn encode_raw(self, storage: ValueStorage) -> Result<u64> {
        let raw = match (self, storage.kind, storage.bits) {
            (Self::Unsigned(value), ValueKind::Unsigned, bits) => {
                let max = max_unsigned(bits);
                if value > max {
                    bail!("value {value} does not fit in {bits} bits");
                }
                value
            }
            (Self::Signed(value), ValueKind::Signed, bits) => {
                let (min, max) = signed_range(bits);
                if value < min || value > max {
                    bail!("value {value} does not fit in {bits} bits");
                }
                (value as i128 & ((1i128 << bits.min(63)) - 1)) as u64
            }
            (Self::Float(value), ValueKind::Float, 32) => u64::from((value as f32).to_bits()),
            (Self::Float(value), ValueKind::Float, 64) => value.to_bits(),
            (actual, _, _) => bail!("type mismatch while encoding {actual}"),
        };

        Ok(raw)
    }

    pub fn decode_raw(storage: ValueStorage, raw: u64) -> Self {
        match storage.kind {
            ValueKind::Unsigned => Self::Unsigned(raw),
            ValueKind::Signed => Self::Signed(sign_extend(raw, storage.bits)),
            ValueKind::Float if storage.bits == 32 => {
                Self::Float(f32::from_bits(raw as u32) as f64)
            }
            ValueKind::Float => Self::Float(f64::from_bits(raw)),
        }
    }
}

struct ParsedSignal {
    name: String,
    start_bit: u16,
    bit_len: u16,
    byte_order: ByteOrder,
    signed: bool,
    factor: f64,
    offset: f64,
    min: Option<f64>,
    max: Option<f64>,
    unit: Option<String>,
    receivers: Vec<String>,
    mux: MuxRole,
}

fn parse_database(path: &Path, raw: &str) -> Result<Database> {
    let board_re = Regex::new(r"^BU_:\s+(?P<nodes>.+)$")?;
    let message_re = Regex::new(
        r"^BO_\s+(?P<id>\d+)\s+(?P<name>[A-Za-z0-9_]+):\s+(?P<dlc>\d+)\s+(?P<sender>\S+)$",
    )?;
    let signal_re = Regex::new(
        r#"^SG_\s+(?P<name>[A-Za-z0-9_]+)\s*(?:(?P<mux>M)|m(?P<mux_value>\d+))?\s*:\s*(?P<start>\d+)\|(?P<len>\d+)@(?P<byte_order>[01])(?P<signed>[+-])\s+\((?P<factor>[-0-9.]+),(?P<offset>[-0-9.]+)\)\s+\[(?P<min>[-0-9.]+)\|(?P<max>[-0-9.]+)\]\s+"(?P<unit>[^"]*)"\s+(?P<receivers>.+)$"#,
    )?;
    let value_table_re =
        Regex::new(r#"^VAL_\s+(?P<msg_id>\d+)\s+(?P<signal>[A-Za-z0-9_]+)\s+(?P<body>.+);$"#)?;
    let value_pair_re = Regex::new(r#"(-?\d+)\s+"([^"]+)""#)?;
    let sig_valtype_re = Regex::new(
        r"^SIG_VALTYPE_\s+(?P<msg_id>\d+)\s+(?P<signal>[A-Za-z0-9_]+)\s+:\s+(?P<kind>\d+);$",
    )?;

    let mut nodes = Vec::new();
    let mut messages = BTreeMap::<u32, MessageDef>::new();
    let mut current_message_id = None;
    let mut raw_value_tables = HashMap::<(u32, String), BTreeMap<i64, String>>::new();
    let mut sig_valtypes = HashMap::<(u32, String), i32>::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(captures) = board_re.captures(trimmed) {
            nodes = captures["nodes"]
                .split_whitespace()
                .map(ToOwned::to_owned)
                .collect();
            continue;
        }

        if let Some(captures) = message_re.captures(trimmed) {
            let id = captures["id"].parse::<u32>()?;
            let message = MessageDef {
                id,
                name: captures["name"].to_string(),
                sender: captures["sender"].to_string(),
                signals: Vec::new(),
                value_tables: HashMap::new(),
            };
            messages.insert(id, message);
            current_message_id = Some(id);
            continue;
        }

        if let Some(captures) = signal_re.captures(trimmed) {
            let message_id = current_message_id.context("signal found before message")?;
            let signal = ParsedSignal {
                name: captures["name"].to_string(),
                start_bit: captures["start"].parse::<u16>()?,
                bit_len: captures["len"].parse::<u16>()?,
                byte_order: match &captures["byte_order"] {
                    "1" => ByteOrder::LittleEndian,
                    _ => ByteOrder::BigEndian,
                },
                signed: &captures["signed"] == "-",
                factor: captures["factor"].parse::<f64>()?,
                offset: captures["offset"].parse::<f64>()?,
                min: parse_optional_number(&captures["min"]),
                max: parse_optional_number(&captures["max"]),
                unit: parse_optional_string(&captures["unit"]),
                receivers: captures["receivers"]
                    .split(',')
                    .map(|part| part.trim().to_string())
                    .collect(),
                mux: if captures.name("mux").is_some() {
                    MuxRole::Multiplexor
                } else if let Some(mux_value) = captures.name("mux_value") {
                    MuxRole::Multiplexed(mux_value.as_str().parse::<u16>()?)
                } else {
                    MuxRole::None
                },
            };
            let value_storage = ValueStorage::from_signal(
                &signal,
                sig_valtypes
                    .get(&(message_id, signal.name.clone()))
                    .copied(),
            );
            let message = messages
                .get_mut(&message_id)
                .context("current message disappeared while parsing")?;
            message.signals.push(SignalDef {
                name: signal.name,
                start_bit: signal.start_bit,
                bit_len: signal.bit_len,
                byte_order: signal.byte_order,
                signed: signal.signed,
                factor: signal.factor,
                offset: signal.offset,
                min: signal.min,
                max: signal.max,
                unit: signal.unit,
                receivers: signal.receivers,
                mux: signal.mux,
                value_storage,
            });
            continue;
        }

        if let Some(captures) = value_table_re.captures(trimmed) {
            let message_id = captures["msg_id"].parse::<u32>()?;
            let signal = captures["signal"].to_string();
            let mut table = BTreeMap::new();
            for pair in value_pair_re.captures_iter(&captures["body"]) {
                table.insert(pair[1].parse::<i64>()?, pair[2].to_string());
            }
            raw_value_tables.insert((message_id, signal), table);
            continue;
        }

        if let Some(captures) = sig_valtype_re.captures(trimmed) {
            sig_valtypes.insert(
                (
                    captures["msg_id"].parse::<u32>()?,
                    captures["signal"].to_string(),
                ),
                captures["kind"].parse::<i32>()?,
            );
        }
    }

    for ((message_id, signal_name), table) in raw_value_tables {
        if let Some(message) = messages.get_mut(&message_id) {
            message.value_tables.insert(signal_name, table);
        }
    }

    for (message_id, message) in &mut messages {
        for signal in &mut message.signals {
            signal.value_storage = ValueStorage::from_signal(
                &ParsedSignal {
                    name: signal.name.clone(),
                    start_bit: signal.start_bit,
                    bit_len: signal.bit_len,
                    byte_order: signal.byte_order,
                    signed: signal.signed,
                    factor: signal.factor,
                    offset: signal.offset,
                    min: signal.min,
                    max: signal.max,
                    unit: signal.unit.clone(),
                    receivers: signal.receivers.clone(),
                    mux: signal.mux,
                },
                sig_valtypes
                    .get(&(*message_id, signal.name.clone()))
                    .copied(),
            );
        }
    }

    let mut sdo_boards = Vec::new();
    for message in messages.values() {
        if !message.name.starts_with("SDO") {
            continue;
        }

        let Some(var_ids) = message.value_tables.get("var_id") else {
            continue;
        };

        let mut variables = Vec::new();
        for signal in &message.signals {
            let MuxRole::Multiplexed(var_id) = signal.mux else {
                continue;
            };

            let name = var_ids
                .get(&(var_id as i64))
                .cloned()
                .unwrap_or_else(|| signal.name.clone());
            variables.push(SdoVariableDef {
                id: var_id,
                name,
                storage: signal.value_storage,
                unit: signal.unit.clone(),
            });
        }
        variables.sort_by_key(|variable| variable.id);

        sdo_boards.push(SdoBoardDef {
            name: message.sender.clone(),
            message_id: message.id,
            variables,
        });
    }
    sdo_boards.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(Database {
        path: path.to_path_buf(),
        nodes,
        messages,
        sdo_boards,
    })
}

fn parse_optional_number(raw: &str) -> Option<f64> {
    if raw.eq_ignore_ascii_case("none") {
        None
    } else {
        raw.parse().ok()
    }
}

fn parse_optional_string(raw: &str) -> Option<String> {
    if raw.is_empty() || raw.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(raw.to_string())
    }
}

fn normalize_storage_bits(bits: u16) -> u16 {
    match bits {
        0..=8 => 8,
        9..=16 => 16,
        17..=32 => 32,
        _ => 64,
    }
}

fn max_unsigned(bits: u16) -> u64 {
    match bits {
        0 => 0,
        64.. => u64::MAX,
        _ => (1u64 << bits) - 1,
    }
}

fn signed_range(bits: u16) -> (i64, i64) {
    let bits = bits.min(63);
    let max = (1i64 << (bits - 1)) - 1;
    let min = -(1i64 << (bits - 1));
    (min, max)
}

fn sign_extend(raw: u64, bits: u16) -> i64 {
    if bits == 0 || bits >= 64 {
        return raw as i64;
    }
    let sign_bit = 1u64 << (bits - 1);
    if raw & sign_bit == 0 {
        raw as i64
    } else {
        (raw | (!0u64 << bits)) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sdo_boards_and_float_valtypes() {
        let database = Database::load(Path::new("dbc/can2.dbc")).expect("dbc should parse");
        let pcu = database
            .sdo_boards
            .iter()
            .find(|board| board.name == "PCU")
            .expect("PCU board missing");

        let kp_batt = pcu
            .variables
            .iter()
            .find(|variable| variable.name == "kp_batt")
            .expect("kp_batt missing");
        assert_eq!(kp_batt.storage.kind, ValueKind::Float);
        assert_eq!(kp_batt.storage.bits, 32);

        let pump_l_max = pcu
            .variables
            .iter()
            .find(|variable| variable.name == "pump_l_max")
            .expect("pump_l_max missing");
        assert_eq!(pump_l_max.storage.kind, ValueKind::Unsigned);
        assert_eq!(pump_l_max.storage.bits, 8);
    }
}
