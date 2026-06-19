use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use crate::can::{CanFrame, CanTransport};
use crate::dbc::{Database, SdoBoardDef, SdoVariableDef, Value, ValueStorage};

pub const OPCODE_GET_REQ: u64 = 1;
pub const OPCODE_SET_REQ: u64 = 2;
pub const OPCODE_RES: u64 = 128;
pub const OPCODE_ERR_OUT_OF_RANGE: u64 = 253;
pub const OPCODE_ERR_WRITE_RO: u64 = 254;
pub const OPCODE_ERR: u64 = 255;

const POLL_SLEEP: Duration = Duration::from_millis(5);

#[derive(Debug, Clone)]
pub struct VariableRef<'a> {
    pub board: &'a SdoBoardDef,
    pub variable: &'a SdoVariableDef,
}

pub fn board_by_name<'a>(database: &'a Database, board_name: &str) -> Result<&'a SdoBoardDef> {
    database
        .sdo_boards
        .iter()
        .find(|board| board.name == board_name)
        .with_context(|| format!("scheda SDO '{board_name}' non trovata"))
}

pub fn variable_by_name<'a>(
    database: &'a Database,
    board_name: &str,
    variable_name: &str,
) -> Result<VariableRef<'a>> {
    let board = board_by_name(database, board_name)?;
    let variable = board
        .variables
        .iter()
        .find(|variable| variable.name == variable_name)
        .with_context(|| format!("variabile '{variable_name}' non trovata su '{board_name}'"))?;
    Ok(VariableRef { board, variable })
}

pub fn send_get(transport: &CanTransport, board: &SdoBoardDef, var_id: u16) -> Result<()> {
    let mut payload = [0u8; 7];
    insert_bits(&mut payload, 0, 8, OPCODE_GET_REQ);
    insert_bits(&mut payload, 8, 10, var_id as u64);
    transport.write_frame(board.message_id, &payload)
}

pub fn send_set(
    transport: &CanTransport,
    board: &SdoBoardDef,
    variable: &SdoVariableDef,
    value: Value,
) -> Result<()> {
    let raw = value.encode_raw(variable.storage)?;
    let mut payload = [0u8; 7];
    insert_bits(&mut payload, 0, 8, OPCODE_SET_REQ);
    insert_bits(&mut payload, 8, 10, variable.id as u64);
    insert_bits(&mut payload, 24, variable.storage.bits, raw);
    transport.write_frame(board.message_id, &payload)
}

pub fn get_with_response(
    transport: &CanTransport,
    board: &SdoBoardDef,
    variable: &SdoVariableDef,
    timeout: Duration,
) -> Result<Value> {
    send_get(transport, board, variable.id)?;
    wait_response(transport, board, variable, timeout)
}

pub fn set_with_ack(
    transport: &CanTransport,
    board: &SdoBoardDef,
    variable: &SdoVariableDef,
    value: Value,
    timeout: Duration,
) -> Result<Value> {
    send_set(transport, board, variable, value)?;
    wait_response(transport, board, variable, timeout)
}

fn wait_response(
    transport: &CanTransport,
    board: &SdoBoardDef,
    variable: &SdoVariableDef,
    timeout: Duration,
) -> Result<Value> {
    let deadline = Instant::now() + timeout;

    loop {
        if let Some(frame) = transport.read_frame()? {
            if let Some(value) = maybe_decode_response(&frame, board, variable)? {
                return Ok(value);
            }
        } else if Instant::now() >= deadline {
            bail!(
                "timeout in attesa risposta {}.{} entro {} ms",
                board.name,
                variable.name,
                timeout.as_millis()
            );
        } else {
            thread::sleep(POLL_SLEEP);
        }
    }
}

fn maybe_decode_response(
    frame: &CanFrame,
    board: &SdoBoardDef,
    variable: &SdoVariableDef,
) -> Result<Option<Value>> {
    if frame.id != board.message_id {
        return Ok(None);
    }

    let opcode = extract_bits(&frame.data, 0, 8);
    let var_id = extract_bits(&frame.data, 8, 10) as u16;
    if var_id != variable.id {
        return Ok(None);
    }

    match opcode {
        OPCODE_RES => {
            let raw = extract_bits(&frame.data, 24, variable.storage.bits);
            Ok(Some(Value::decode_raw(variable.storage, raw)))
        }
        OPCODE_ERR_OUT_OF_RANGE => Err(anyhow!(
            "{}.{} fuori range",
            board.name,
            variable.name
        )),
        OPCODE_ERR_WRITE_RO => Err(anyhow!(
            "{}.{} readonly",
            board.name,
            variable.name
        )),
        OPCODE_ERR => Err(anyhow!("{}.{} errore generico", board.name, variable.name)),
        other => Err(anyhow!(
            "{}.{} opcode inatteso {}",
            board.name,
            variable.name,
            other
        )),
    }
}

pub fn parse_cli_value(value: &str, storage: ValueStorage) -> Result<Value> {
    Value::parse(value, storage)
}

pub fn insert_bits(buffer: &mut [u8], start_bit: u16, bit_len: u16, value: u64) {
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

pub fn extract_bits(buffer: &[u8], start_bit: u16, bit_len: u16) -> u64 {
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
