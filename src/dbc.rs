use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::generated::{self, MessageInfo, SignalInfo};

#[derive(Debug, Clone)]
pub struct Database {
    pub path: PathBuf,
    pub nodes: Vec<String>,
    pub messages: BTreeMap<u32, MessageDef>,
    pub sdo_boards: Vec<SdoBoardDef>,
}

#[derive(Debug, Clone)]
pub struct MessageDef {
    pub name: String,
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
    /// Type exposed to the user by the generated 2rust API.
    pub storage: ValueStorage,
    /// Representation carried in the SDO payload before DBC scaling.
    pub wire_storage: ValueStorage,
    pub factor: f64,
    pub offset: f64,
    pub unit: Option<String>,
    pub enum_values: BTreeMap<i64, String>,
}

#[derive(Debug, Clone)]
pub enum Value {
    Unsigned(u64),
    Signed(i64),
    Float(f64),
}

impl Database {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read(path)
            .with_context(|| format!("failed to read dbc file {}", path.display()))?;
        if raw != generated::DBC_SOURCE {
            bail!(
                "il DBC {} non coincide con il codec 2rust incorporato da {}; \
                 ricompila con SDODPS_DBC_PATH={} cargo build",
                path.display(),
                generated::DBC_SOURCE_PATH,
                path.display()
            );
        }

        let annotations = DbcAnnotations::parse(
            std::str::from_utf8(&raw)
                .with_context(|| format!("DBC {} non valido UTF-8", path.display()))?,
        )?;
        Ok(Self::from_generated(path, annotations))
    }

    fn from_generated(path: &Path, annotations: DbcAnnotations) -> Self {
        let mut messages = BTreeMap::new();
        let mut sdo_boards = Vec::new();

        for message in generated::get_all_mess() {
            let sender = annotations
                .senders
                .get(&message.id)
                .cloned()
                .unwrap_or_else(|| inferred_sender(message.name));
            messages.insert(
                message.id,
                MessageDef {
                    name: message.name.to_string(),
                },
            );

            if let Some(board) = sdo_board(message, sender, &annotations.value_tables) {
                sdo_boards.push(board);
            }
        }
        sdo_boards.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then(left.message_id.cmp(&right.message_id))
        });

        Self {
            path: path.to_path_buf(),
            nodes: annotations.nodes,
            messages,
            sdo_boards,
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
    pub fn integer_value(&self) -> Option<i64> {
        match self {
            Self::Unsigned(value) => i64::try_from(*value).ok(),
            Self::Signed(value) => Some(*value),
            Self::Float(_) => None,
        }
    }

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
                if bits >= 64 {
                    value as u64
                } else {
                    (value as u64) & max_unsigned(bits)
                }
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

impl SdoVariableDef {
    pub fn decode_raw(&self, raw: u64) -> Value {
        let wire_value = Value::decode_raw(self.wire_storage, raw);
        if !self.is_scaled() {
            return wire_value;
        }

        let numeric = match wire_value {
            Value::Unsigned(value) => value as f64,
            Value::Signed(value) => value as f64,
            Value::Float(value) => value,
        };
        Value::Float(numeric * self.factor + self.offset)
    }

    pub fn encode_raw(&self, value: Value) -> Result<u64> {
        if !self.is_scaled() {
            return value.encode_raw(self.wire_storage);
        }

        let Value::Float(physical) = value else {
            bail!("type mismatch while encoding scaled value");
        };
        if !physical.is_finite() {
            bail!("value {physical} is not finite");
        }
        if self.factor == 0.0 {
            bail!("cannot encode {}: DBC scaling is zero", self.name);
        }

        let wire = (physical - self.offset) / self.factor;
        let wire_value = match self.wire_storage.kind {
            ValueKind::Unsigned => {
                let rounded = wire.round();
                if rounded < 0.0 || rounded > u64::MAX as f64 {
                    bail!("value {physical} is outside the wire range");
                }
                Value::Unsigned(rounded as u64)
            }
            ValueKind::Signed => {
                let rounded = wire.round();
                if rounded < i64::MIN as f64 || rounded > i64::MAX as f64 {
                    bail!("value {physical} is outside the wire range");
                }
                Value::Signed(rounded as i64)
            }
            ValueKind::Float => Value::Float(wire),
        };
        wire_value.encode_raw(self.wire_storage)
    }

    fn is_scaled(&self) -> bool {
        self.factor != 1.0 || self.offset != 0.0
    }
}

fn sdo_board(
    message: &'static MessageInfo,
    sender: String,
    value_tables: &HashMap<(u32, String), BTreeMap<i64, String>>,
) -> Option<SdoBoardDef> {
    if !message.name.starts_with("SDO")
        || !message.signals.iter().any(|signal| signal.name == "opcode")
        || !message.signals.iter().any(|signal| signal.name == "flags")
        || !message
            .signals
            .iter()
            .any(|signal| signal.name == "dbc_hash")
        || !message
            .signals
            .iter()
            .any(|signal| signal.name == "var_id" && signal.multiplexor)
    {
        return None;
    }

    let mut variables = message
        .signals
        .iter()
        .filter_map(|signal| {
            let id = u16::try_from(signal.switch_value?).ok()?;
            (signal.multiplexed && signal.start_bit == 24 && signal.bit_length > 0).then(|| {
                SdoVariableDef {
                    id,
                    name: signal.name.to_string(),
                    storage: physical_storage(signal),
                    wire_storage: wire_storage(signal),
                    factor: signal.scaling,
                    offset: signal.offset,
                    unit: (!signal.units.is_empty()).then(|| signal.units.to_string()),
                    enum_values: value_tables
                        .get(&(message.id, signal.name.to_string()))
                        .cloned()
                        .unwrap_or_default(),
                }
            })
        })
        .collect::<Vec<_>>();
    variables.sort_by_key(|variable| variable.id);
    (!variables.is_empty()).then_some(SdoBoardDef {
        name: sender,
        message_id: message.id,
        variables,
    })
}

fn physical_storage(signal: &SignalInfo) -> ValueStorage {
    if signal.floating || signal.scaling != 1.0 || signal.offset != 0.0 {
        ValueStorage {
            kind: ValueKind::Float,
            bits: if signal.bit_length <= 32 { 32 } else { 64 },
        }
    } else {
        ValueStorage {
            kind: if signal.signed {
                ValueKind::Signed
            } else {
                ValueKind::Unsigned
            },
            bits: normalize_storage_bits(signal.bit_length),
        }
    }
}

fn wire_storage(signal: &SignalInfo) -> ValueStorage {
    ValueStorage {
        kind: if signal.floating {
            ValueKind::Float
        } else if signal.signed {
            ValueKind::Signed
        } else {
            ValueKind::Unsigned
        },
        bits: signal.bit_length,
    }
}

fn inferred_sender(message_name: &str) -> String {
    message_name
        .strip_prefix("SDO")
        .filter(|name| !name.is_empty())
        .unwrap_or(message_name)
        .to_string()
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
    match bits {
        0 => (0, 0),
        64.. => (i64::MIN, i64::MAX),
        _ => {
            let max = (1i64 << (bits - 1)) - 1;
            (-max - 1, max)
        }
    }
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

#[derive(Default)]
struct DbcAnnotations {
    nodes: Vec<String>,
    senders: HashMap<u32, String>,
    value_tables: HashMap<(u32, String), BTreeMap<i64, String>>,
}

impl DbcAnnotations {
    fn parse(raw: &str) -> Result<Self> {
        let mut parsed = Self::default();
        let mut named_tables = HashMap::<String, BTreeMap<i64, String>>::new();
        let mut table_refs = Vec::<(u32, String, String)>::new();

        for line in raw.lines().map(str::trim) {
            if let Some(nodes) = line.strip_prefix("BU_:") {
                parsed.nodes = nodes.split_whitespace().map(str::to_string).collect();
            } else if let Some(message) = line.strip_prefix("BO_ ") {
                let mut fields = message.split_whitespace();
                let Some(id) = fields.next() else { continue };
                let _name = fields.next();
                let _dlc = fields.next();
                let Some(sender) = fields.next() else {
                    continue;
                };
                parsed.senders.insert(id.parse()?, sender.to_string());
            } else if let Some(table) = line.strip_prefix("VAL_TABLE_ ") {
                let Some((name, body)) = table.split_once(char::is_whitespace) else {
                    continue;
                };
                named_tables.insert(name.to_string(), parse_value_pairs(body)?);
            } else if let Some(value) = line.strip_prefix("VAL_ ") {
                let mut fields = value.splitn(3, char::is_whitespace);
                let (Some(id), Some(signal), Some(body)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    continue;
                };
                let message_id = id.parse::<u32>()?;
                let values = parse_value_pairs(body)?;
                if values.is_empty() {
                    let table_name = body.trim().trim_end_matches(';').trim();
                    if !table_name.is_empty() {
                        table_refs.push((message_id, signal.to_string(), table_name.to_string()));
                    }
                } else {
                    parsed
                        .value_tables
                        .insert((message_id, signal.to_string()), values);
                }
            }
        }

        for (message_id, signal, table_name) in table_refs {
            if let Some(table) = named_tables.get(&table_name) {
                parsed
                    .value_tables
                    .insert((message_id, signal), table.clone());
            }
        }
        Ok(parsed)
    }
}

fn parse_value_pairs(input: &str) -> Result<BTreeMap<i64, String>> {
    let mut values = BTreeMap::new();
    let mut rest = input.trim();

    while !rest.is_empty() {
        rest = rest.trim_start();
        if rest.starts_with(';') {
            break;
        }
        let number_len = rest
            .char_indices()
            .take_while(|(index, ch)| ch.is_ascii_digit() || (*index == 0 && *ch == '-'))
            .map(|(_, ch)| ch.len_utf8())
            .sum::<usize>();
        if number_len == 0 {
            break;
        }
        let value = rest[..number_len].parse::<i64>()?;
        rest = rest[number_len..].trim_start();
        let Some(quoted) = rest.strip_prefix('"') else {
            break;
        };
        let Some(end_quote) = quoted.find('"') else {
            bail!("unterminated DBC value label in '{input}'");
        };
        values.insert(value, quoted[..end_quote].to_string());
        rest = &quoted[end_quote + 1..];
    }

    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_sdo_layout_from_2rust_metadata() {
        let database = Database::load(Path::new("dbc/can2.dbc")).expect("dbc should load");
        assert_eq!(database.messages.len(), generated::get_all_mess().len());

        let pcu = database
            .sdo_boards
            .iter()
            .find(|board| board.name == "PCU")
            .expect("PCU board missing");
        let sum_factor_air_left = pcu
            .variables
            .iter()
            .find(|variable| variable.name == "sum_factor_air_left")
            .expect("sum_factor_air_left missing");
        assert_eq!(sum_factor_air_left.storage.kind, ValueKind::Float);
        assert_eq!(sum_factor_air_left.storage.bits, 32);

        let mcu_vd = database
            .sdo_boards
            .iter()
            .find(|board| board.message_id == 508)
            .expect("MCU VD board missing");
        let lat_acc = mcu_vd
            .variables
            .iter()
            .find(|variable| variable.name == "dms_tires_lat_acc_mult")
            .expect("dms_tires_lat_acc_mult missing");
        assert_eq!(lat_acc.storage.kind, ValueKind::Float);
        assert_eq!(lat_acc.wire_storage.kind, ValueKind::Unsigned);
        assert_eq!(lat_acc.wire_storage.bits, 8);
        assert_eq!(lat_acc.decode_raw(17).to_string(), "0.8500");
        assert_eq!(
            lat_acc
                .encode_raw(Value::Float(0.85))
                .expect("scaled value should encode"),
            17
        );
    }

    #[test]
    fn preserves_named_enum_labels_from_the_dbc() {
        let database = Database::load(Path::new("dbc/can2.dbc")).expect("dbc should load");
        let mcu = database
            .sdo_boards
            .iter()
            .find(|board| board.message_id == 501)
            .expect("MCU board missing");
        let current_m_mission = mcu
            .variables
            .iter()
            .find(|variable| variable.name == "current_m_mission")
            .expect("current_m_mission missing");

        assert_eq!(
            current_m_mission.enum_values.get(&2),
            Some(&"autocross".to_string())
        );
    }

    #[test]
    fn signed_64_bit_values_round_trip() {
        let storage = ValueStorage {
            kind: ValueKind::Signed,
            bits: 64,
        };
        let raw = Value::Signed(i64::MIN).encode_raw(storage).unwrap();
        assert!(matches!(
            Value::decode_raw(storage, raw),
            Value::Signed(i64::MIN)
        ));
    }
}
