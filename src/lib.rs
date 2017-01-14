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

use std::io;
use tokio_core::io::{Codec, EasyBuf, Io, Framed};
use tokio_proto::pipeline::ClientProto;
use crc16::State;
use byteorder::{BigEndian,ByteOrder};

#[repr(u8)]
#[derive(Clone,Copy,Debug)]
pub enum MeasurementType{
    GridVoltage = 1,
    GridCurrent = 2,
    GridPower = 3,
    Frequency = 4,
    Vbulk = 5,
    IleakDc = 6,
    ILeakInverter = 7,
    Pin1 = 8,
    Pin2 = 9,
    InverterTemperature = 21,
    BoosterTemperature = 22,
    Input1Voltage = 23,
    Input1Current = 25,
    Input2Voltage = 26,
    Input2Current = 27,
    GridVoltageDc = 28,
    GridFrequencyDc = 29,
    IsolationResistance = 30,
    VbulkDc = 31,
    AverageGridVoltage = 32,
    VbulkMid = 33,
    PeakPower = 34,
    PeakPowerToday = 35,
    GridVoltageNeutral = 36,
    WindGeneratorFrequency = 37,
    GridVoltageNeutralPhase = 38,
    GridCurrentPhaseR = 39,
    GridCurrentPhaseS = 40,
    GridCurrentPhaseT = 41,
    FrequencyPhaseR = 42,
    FrequencyPhaseS = 43,
    FrequencyPhaseT = 44,
    VbulkPos = 45,
    VbulkNeg = 46,
    SupervisorTemp = 47,
    AlimTemp = 48,
    HeatSinkTemp = 49,
    Temp1 = 50,
    Temp2 = 51,
    Temp3 = 52,
    FanSpeed1 = 53,
    FanSpeed2 = 54,
    FanSpeed3 = 55,
    FanSpeed4 = 56,
    FanSpeed5 = 57,
    PowerSaturationLimit = 58,
    ReferenceRingBulk = 59,
    VpanelMicro = 60,
    GridVoltagePhaseR = 61,
    GridVoltagePhaseS = 62,
    GridVoltagePhaseT = 63,
}

#[repr(u8)]
#[derive(Clone,Copy,Debug)]
pub enum CumulativeDuration{
    Daily =0 ,
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
        global:u8
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
        global: u8,
        inverter: u8,
        dc1: u8,
        dc2: u8,
        alarm: u8
    },
    PartNumber([u8; 6]),
    Version{
        trans: u8,
        global: u8,
        par1: u8,
        par2: u8,
        par3: u8,
        par4: u8
    },
    Measure{
        trans:  u8,
        global: u8,
        val: f32,
        type_: MeasurementType,
    },
    SerialNumber([u8;6]),
    ManufactureDate{
        trans:u8,
        global:u8,
        week: [u8;2],
        year: [u8;2]
    },
    //Some skipped
    CumulativeEnergy{
        trans: u8,
        global: u8,
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
        println!("got {} bytes", buf.len());
        if buf.len() >= 8 {
            let packet = buf.drain_to(8);
            //CRC check
            let data = &packet.as_slice()[0..6];
            let crc_val = &packet.as_slice()[6..8];
            let crc_calc = State::<AURORA_CRC>::calculate(data);
            if crc_val != &[lo(crc_calc),hi(crc_calc)] {
                return Err(io::Error::new(io::ErrorKind::Other,"CRC mismatch"))
            }
            //Parse
            if let Some(last) = self.last_request.take(){
                Ok(Some(match last {
                    Request::State => Response::State{
                        global: data[0],
                        inverter: data[1],
                        dc1: data[2],
                        dc2: data[3],
                        alarm: data[4]
                    },
                    Request::PartNumber => Response::PartNumber([data[0],data[1],data[2],data[3],data[4],data[5]]),
                    Request::Version => Response::Version{
                        trans: data[0],
                        global: data[1],
                        par1: data[2],
                        par2: data[3],
                        par3: data[4],
                        par4: data[5]
                    },
                    Request::Measure{type_,global} => Response::Measure{
                        trans: data[0],
                        global: data[1],
                        val: BigEndian::read_f32(&data[2..]),
                        type_: type_
                    },
                    Request::SerialNumber => Response::SerialNumber([data[0],data[1],data[2],data[3],data[4],data[5]]),
                    Request::ManufactureDate => Response::ManufactureDate{
                        trans: data[0],
                        global: data[1],
                        week: [data[2],data[3]],
                        year: [data[4],data[5]],
                    },
                    Request::CumulativeEnergy(duration) => Response::CumulativeEnergy{
                        trans: data[0],
                        global: data[1],
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
                    data[3] = global;
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
            State::<AURORA_CRC>::calculate(data)
        };
        encoded[8] = lo(crc);
        encoded[9] = hi(crc);
        self.last_request = Some(msg);
        buf.extend_from_slice(&encoded);
        println!("send buffer: {:?}",buf);
        Ok(())
    }

}

pub struct AuroraProto;

impl<T: Io + 'static> ClientProto<T> for AuroraProto{
    type Request = (u8,Request);
    type Response = Response;
    type Transport = Framed<T, AuroraCodec>;
    type BindTransport = Result<Self::Transport, io::Error>;
    fn bind_transport(&self, io: T) -> Self::BindTransport {
        Ok(io.framed(AuroraCodec{last_request:None}))
    }
}

type AURORA_CRC = crc16::CCITT_FALSE;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
    }
}
