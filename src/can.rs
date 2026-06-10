use std::ffi::CString;
use std::io;
use std::mem::{MaybeUninit, size_of};
use std::os::fd::RawFd;

use anyhow::{Context, Result, bail};

const AF_CAN: i32 = 29;
const PF_CAN: i32 = AF_CAN;
const CAN_RAW: i32 = 1;
const CAN_EFF_FLAG: u32 = 0x8000_0000;

#[repr(C)]
struct SockAddrCan {
    can_family: libc::sa_family_t,
    can_ifindex: libc::c_int,
    addr: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CanFrameRaw {
    can_id: u32,
    can_dlc: u8,
    __pad: u8,
    __res0: u8,
    __res1: u8,
    data: [u8; 8],
}

#[derive(Debug, Clone)]
pub struct CanFrame {
    pub id: u32,
    pub data: [u8; 8],
}

pub struct CanTransport {
    fd: RawFd,
}

impl CanTransport {
    pub fn open(interface: &str) -> Result<Self> {
        let name = CString::new(interface).context("interface contains interior NUL")?;

        let fd = unsafe { libc::socket(PF_CAN, libc::SOCK_RAW | libc::SOCK_NONBLOCK, CAN_RAW) };
        if fd < 0 {
            return Err(io::Error::last_os_error()).context("socket(PF_CAN) failed");
        }

        let if_index = unsafe { libc::if_nametoindex(name.as_ptr()) };
        if if_index == 0 {
            let error = io::Error::last_os_error();
            unsafe {
                libc::close(fd);
            }
            return Err(error).with_context(|| format!("if_nametoindex('{interface}') failed"));
        }

        let address = SockAddrCan {
            can_family: AF_CAN as libc::sa_family_t,
            can_ifindex: if_index as i32,
            addr: [0; 8],
        };

        let bind_result = unsafe {
            libc::bind(
                fd,
                (&address as *const SockAddrCan).cast::<libc::sockaddr>(),
                size_of::<SockAddrCan>() as libc::socklen_t,
            )
        };
        if bind_result < 0 {
            let error = io::Error::last_os_error();
            unsafe {
                libc::close(fd);
            }
            return Err(error).with_context(|| format!("bind('{interface}') failed"));
        }

        Ok(Self { fd })
    }

    pub fn read_frame(&self) -> Result<Option<CanFrame>> {
        let mut raw = MaybeUninit::<CanFrameRaw>::zeroed();
        let result = unsafe {
            libc::read(
                self.fd,
                raw.as_mut_ptr().cast::<libc::c_void>(),
                size_of::<CanFrameRaw>(),
            )
        };

        if result < 0 {
            let error = io::Error::last_os_error();
            if matches!(error.kind(), io::ErrorKind::WouldBlock) {
                return Ok(None);
            }
            return Err(error).context("read(can_frame) failed");
        }

        if result as usize != size_of::<CanFrameRaw>() {
            bail!("short CAN frame read: {} bytes", result);
        }

        let raw = unsafe { raw.assume_init() };
        Ok(Some(CanFrame {
            id: raw.can_id & !CAN_EFF_FLAG,
            data: raw.data,
        }))
    }

    pub fn write_frame(&self, id: u32, bytes: &[u8]) -> Result<()> {
        if bytes.len() > 8 {
            bail!("CAN frame payload too large: {}", bytes.len());
        }

        let mut data = [0u8; 8];
        data[..bytes.len()].copy_from_slice(bytes);
        let raw = CanFrameRaw {
            can_id: id,
            can_dlc: bytes.len() as u8,
            __pad: 0,
            __res0: 0,
            __res1: 0,
            data,
        };

        let result = unsafe {
            libc::write(
                self.fd,
                (&raw as *const CanFrameRaw).cast::<libc::c_void>(),
                size_of::<CanFrameRaw>(),
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error()).context("write(can_frame) failed");
        }
        if result as usize != size_of::<CanFrameRaw>() {
            bail!("short CAN frame write: {} bytes", result);
        }

        Ok(())
    }
}

impl Drop for CanTransport {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}
