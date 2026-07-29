include!(env!("SDODPS_GENERATED_WRAPPER_RS"));

pub use codec::{
    CanFrame, DBCC_GENERATOR_VERSION, DBCC_HASH, DBCC_MODULE_NAME, MessageInfo, SdoOpcode,
    SignalInfo, get_all_mess, sdo_frame,
};

pub const DBC_SOURCE: &[u8] = include_bytes!(env!("SDODPS_DBC_SOURCE"));
pub const DBC_SOURCE_PATH: &str = env!("SDODPS_DBC_SOURCE");
