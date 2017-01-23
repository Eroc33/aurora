//! Module for interfacing with the serial comms port on most aurora inverters
//!
//! Spec can be found at:
//! [xilinx forum](https://forums.xilinx.com/xlnx/attachments/xlnx/CONN/10023/1/AuroraCommunicationProtocol_4_2.pdf)

extern crate futures;
extern crate tokio_core;
extern crate tokio_proto;
extern crate tokio_service;
extern crate crc16;
extern crate byteorder;
#[macro_use]
extern crate enum_primitive;

mod state_codes;
pub use state_codes::*;


use std::io;
use std::result::Result as StdResult;

use tokio_core::io::{Codec, EasyBuf, Io, Framed};
use tokio_proto::pipeline::ClientProto;
use crc16::State;
use byteorder::{BigEndian,ByteOrder};
use enum_primitive::FromPrimitive;


#[repr(u8)]
#[derive(Clone,Copy,Debug)]
pub enum CumulativeDuration{
    Daily = 0,
    Weekly = 1,
    //2 intentionally skipped
    Montly = 3,
    Yearly = 4,
    Total = 5,
    SinceReset = 6,
}

#[derive(Debug)]
pub enum Request{
    State,
    PartNumber,
    Version,
    Measure{
        type_: MeasurementType,
        global:bool
    },
    SerialNumber,
    ManufactureDate,
    //Some skipped
    CumulativeEnergy(CumulativeDuration),
    //TODO: MORE...
}

#[derive(Debug)]
pub enum Response{
    State{
        trans: TransmissionState,
        global: GlobalState,
        inverter: InverterState,
        dc1: DcDcState,
        dc2: DcDcState,
        alarm: u8
    },
    PartNumber([u8; 6]),
    Version{
        trans: TransmissionState,
        global: GlobalState,
        par1: u8,
        par2: u8,
        par3: u8,
        par4: u8
    },
    Measure{
        trans: TransmissionState,
        global: GlobalState,
        val: f32,
        type_: MeasurementType,
    },
    SerialNumber([u8;6]),
    ManufactureDate{
        trans: TransmissionState,
        global: GlobalState,
        week: [u8;2],
        year: [u8;2]
    },
    //Some skipped
    CumulativeEnergy{
        trans: TransmissionState,
        global: GlobalState,
        value: u32,
        duration: CumulativeDuration
    },
    //TODO: MORE...
}

#[inline]
fn lo(val: u16) -> u8
{
    (val & 0xFF) as u8
}

#[inline]
fn hi(val: u16) -> u8
{
    ((val >> 8) & 0xFF) as u8
}

pub struct AuroraCodec{
    last_request: Option<Request>,
}

impl Codec for AuroraCodec{
    type In = Response;
    type Out = (u8,Request);
    fn decode(&mut self, buf: &mut EasyBuf) -> io::Result<Option<Self::In>>
    {
        if buf.len() >= 8 {
            let packet = buf.drain_to(8);
            //CRC check
            let data = &packet.as_slice()[0..6];
            let crc_val = &packet.as_slice()[6..8];
            let crc_calc = State::<AuroraCrc>::calculate(data);
            if crc_val != &[lo(crc_calc),hi(crc_calc)] {
                return Err(io::Error::new(io::ErrorKind::Other,"CRC mismatch"))
            }
            //Parse
            if let Some(last) = self.last_request.take(){
                Ok(Some(match last {
                    Request::State => Response::State{
                        trans: TransmissionState::from_u8(data[0]).unwrap(),
                        global: GlobalState::from_u8(data[1]).unwrap(),
                        inverter: InverterState::from_u8(data[2]).unwrap(),
                        dc1: DcDcState::from_u8(data[3]).unwrap(),
                        dc2: DcDcState::from_u8(data[4]).unwrap(),
                        alarm: data[5]
                    },
                    Request::PartNumber => Response::PartNumber([data[0],data[1],data[2],data[3],data[4],data[5]]),
                    Request::Version => Response::Version{
                        trans: TransmissionState::from_u8(data[0]).unwrap(),
                        global: GlobalState::from_u8(data[1]).unwrap(),
                        par1: data[2],
                        par2: data[3],
                        par3: data[4],
                        par4: data[5]
                    },
                    Request::Measure{type_,..} => Response::Measure{
                        trans: TransmissionState::from_u8(data[0]).unwrap(),
                        global: GlobalState::from_u8(data[1]).unwrap(),
                        val: BigEndian::read_f32(&data[2..]),
                        type_: type_
                    },
                    Request::SerialNumber => Response::SerialNumber([data[0],data[1],data[2],data[3],data[4],data[5]]),
                    Request::ManufactureDate => Response::ManufactureDate{
                        trans: TransmissionState::from_u8(data[0]).unwrap(),
                        global: GlobalState::from_u8(data[1]).unwrap(),
                        week: [data[2],data[3]],
                        year: [data[4],data[5]],
                    },
                    Request::CumulativeEnergy(duration) => Response::CumulativeEnergy{
                        trans: TransmissionState::from_u8(data[0]).unwrap(),
                        global: GlobalState::from_u8(data[1]).unwrap(),
                        value: BigEndian::read_u32(&data[2..]),
                        duration: duration,
                    }
                }))
            }else{
                Err(io::Error::new(io::ErrorKind::Other,"Got response without request"))
            }

        }else{
            Ok(None)
        }
    }
    fn encode(&mut self, (addr,msg): Self::Out, buf: &mut Vec<u8>)-> io::Result<()>
    {
        let mut encoded:[u8;10] = [0;10];
        encoded[0] = addr;
        let crc = {
            let data = &mut encoded[0..8];
            match msg{
                Request::State => {
                    data[1] = 50;
                }
                Request::PartNumber => {
                    data[1] = 52;
                }
                Request::Version => {
                    data[1] = 58;
                }
                Request::Measure{type_,global} =>{
                    data[1] = 59;
                    data[2] = type_ as u8;
                    data[3] = if global {1}else{0};
                }
                Request::SerialNumber => {
                    data[1] = 63;
                },
                Request::ManufactureDate => {
                    data[1] = 65;
                },
                Request::CumulativeEnergy(ref duration) => {
                    data[1] = 78;
                    data[2] = *duration as u8;
                }
            }
            State::<AuroraCrc>::calculate(data)
        };
        encoded[8] = lo(crc);
        encoded[9] = hi(crc);
        self.last_request = Some(msg);
        buf.extend_from_slice(&encoded);
        Ok(())
    }

}

pub struct AuroraProto;

impl<T: Io + 'static> ClientProto<T> for AuroraProto{
    type Request = (u8,Request);
    type Response = Response;
    type Transport = Framed<T, AuroraCodec>;
    type BindTransport = StdResult<Self::Transport, io::Error>;
    fn bind_transport(&self, io: T) -> Self::BindTransport {
        Ok(io.framed(AuroraCodec{last_request:None}))
    }
}

type AuroraCrc = crc16::X_25;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
    }
}
