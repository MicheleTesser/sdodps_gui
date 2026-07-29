use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use crate::can::{CanFrame, CanTransport};
use crate::dbc::{Database, SdoBoardDef, SdoVariableDef, Value, ValueStorage};
use crate::generated::{SdoOpcode, sdo_frame};

pub const OPCODE_RES: u64 = SdoOpcode::Response as u64;
pub const OPCODE_ERR_OUT_OF_RANGE: u64 = SdoOpcode::ErrOutOfRange as u64;
pub const OPCODE_ERR_WRITE_RO: u64 = SdoOpcode::ErrWriteReadOnly as u64;
pub const OPCODE_ERR: u64 = SdoOpcode::Error as u64;

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
    write_generated_frame(
        transport,
        sdo_frame(board.message_id, SdoOpcode::GetReq, var_id, 0, 0),
    )
}

pub fn send_set(
    transport: &CanTransport,
    board: &SdoBoardDef,
    variable: &SdoVariableDef,
    value: Value,
) -> Result<()> {
    let raw = variable.encode_raw(value)?;
    write_generated_frame(
        transport,
        sdo_frame(
            board.message_id,
            SdoOpcode::SetReq,
            variable.id,
            raw,
            u32::from(variable.wire_storage.bits),
        ),
    )
}

fn write_generated_frame(
    transport: &CanTransport,
    frame: crate::generated::CanFrame,
) -> Result<()> {
    let payload = frame.payload.to_le_bytes();
    transport.write_frame(frame.id, &payload[..usize::from(frame.dlc)])
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
            let raw = extract_bits(&frame.data, 24, variable.wire_storage.bits);
            Ok(Some(variable.decode_raw(raw)))
        }
        OPCODE_ERR_OUT_OF_RANGE => Err(anyhow!("{}.{} fuori range", board.name, variable.name)),
        OPCODE_ERR_WRITE_RO => Err(anyhow!("{}.{} readonly", board.name, variable.name)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_sdo_frame_uses_the_dbc_layout() {
        let frame = sdo_frame(0x1f4, SdoOpcode::SetReq, 0x155, 0x5a, 8);
        let payload = frame.payload.to_le_bytes();

        assert_eq!(frame.id, 0x1f4);
        assert_eq!(frame.dlc, 7);
        assert_eq!(extract_bits(&payload, 0, 8), SdoOpcode::SetReq as u64);
        assert_eq!(extract_bits(&payload, 8, 10), 0x155);
        assert_eq!(extract_bits(&payload, 24, 8), 0x5a);
    }

    #[test]
    fn extracts_only_the_declared_boolean_bit() {
        let mut payload = [0u8; 7];
        payload[3] = 200;
        assert_eq!(extract_bits(&payload, 24, 1), 0);

        payload[3] = 201;
        assert_eq!(extract_bits(&payload, 24, 1), 1);
    }
}
