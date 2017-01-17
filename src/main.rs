#![feature(conservative_impl_trait)]
extern crate aurora_rs;
extern crate futures;
extern crate tokio_core;
extern crate tokio_service;
extern crate tokio_proto;
extern crate tokio_timer;

extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate toml;
extern crate hyper;
extern crate mime;
extern crate chrono;

use aurora_rs as aurora;

use std::time::Duration;
use std::net::SocketAddr;

use futures::{Future,IntoFuture,Stream,Async,Poll};
use tokio_core::reactor::Core;
use tokio_service::Service;
use tokio_proto::TcpClient;
use tokio_timer::{Timer,TimerError,Sleep};

use chrono::{Local};

use hyper::Method;
use hyper::status::StatusCode;
use hyper::client::Request as HttpRequest;

use aurora::{AuroraProto,Request,Response,CumulativeDuration,MeasurementType,ErrorKind};

#[derive(Debug,Clone,Deserialize)]
struct PvOutputConfig{
    ///Pvoutput.org sid
    system_id: String,
    ///Pvoutput.org api key
    api_key: String,
}

#[derive(Debug,Clone,Deserialize)]
struct Config{
    ///The address on which the client will connect to the tcp->serial bridge
    tcp_address: SocketAddr,
    ///The aurora protocol address
    aurora_address: u8,
    ///The time between requests to the inverter
    poll_duration: Duration,
    ///The number of times to wait `poll_duration` before failing
    timeout_mul: u32,
    ///PVOutput.org config
    pv_output: PvOutputConfig
}

//creates a custom timer with a longer tick_duration to allow longer (but marginally less accurate) timeouts
fn timer() -> Timer{
    tokio_timer::wheel().tick_duration(Duration::from_secs(1)).build()
}

///Only runs the underlying stream at certain time intervals
struct RateLimitedStream<S>
{
    //Inner stream to limit
    inner: S,
    //The current sleep
    sleep: Option<Sleep>,
    //Time to sleep between each bout of polling the stream
    delay: Duration,
    //The timer to use for the sleep
    timer: Timer,
    ///Skips some initial limiting if this is positive (useful if folding
    /// and you want an initial value out immediately)
    delay_skips: u64,
}

impl <S> RateLimitedStream<S>{
    pub fn new(inner: S, delay: Duration, timer: Timer, skips: u64) -> Self
    {
        RateLimitedStream{
            inner: inner,
            delay: delay,
            timer: timer,
            sleep: None,
            delay_skips: skips,
        }
    }
}

impl<S> Stream for RateLimitedStream<S>
where S: Stream,
      S::Error: From<TimerError>
{
    type Item = S::Item;
    type Error = S::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error>
    {
        if self.delay_skips > 0 {
            if let Async::Ready(item) = self.inner.poll()? {
                self.delay_skips -= 1;
                return Ok(Async::Ready(item));
            }
        }

        if let Some(Ok(Async::Ready(_))) = self.sleep.as_mut().map(|ref mut s| s.poll()) {
            if let Async::Ready(item) = self.inner.poll()? {
                self.sleep = Some(self.timer.sleep(self.delay));
                return Ok(Async::Ready(item));
            }
        }

        if self.sleep.is_none() {
            self.sleep = Some(self.timer.sleep(self.delay));
        }

        Ok(Async::NotReady)
    }
}

type ResponsePair<S: Service> = (S::Response,S::Response);

fn energy_voltage_stream<S>(client:S,addr:u8) -> impl Stream<Item=(u32,f32),Error=S::Error>
where S: Service<Request=(u8,Request),Response=Response>,
      S::Error: std::fmt::Debug + From<TimerError>
{
    type State<S> = (u8,S);

    fn unfold_energy_voltage_stream<S>((addr,client): State<S>) -> Option<impl IntoFuture<Item=(ResponsePair<S>,State<S>),Error=S::Error>>
        where S: Service<Request=(u8,Request)>,
              S::Error: std::fmt::Debug 
    {
        let res = client.call((addr,Request::CumulativeEnergy(CumulativeDuration::Daily)))
            .map(move |i| (i,client))
            .and_then(move |(energy,client)|{
                let res = client.call((addr,Request::Measure{type_:MeasurementType::Input1Voltage,global:true}));
                res.map(move |i| ((energy,i),(addr,client)))
            });
        Some(res)
    }

    futures::stream::unfold((addr,client),unfold_energy_voltage_stream)
        .filter_map(|res|{
            match res{
                (Response::CumulativeEnergy{value,..},Response::Measure{val,..}) => Some((value,val)),
                _ => None
            }
        })
}

fn load_config() -> std::io::Result<Config>{
    use std::io::Read;
    use serde::Deserialize;
    let mut cfg_file = std::fs::File::open("Config.toml")?;
    let mut file_contents = String::new();
    cfg_file.read_to_string(&mut file_contents)?;
    let mut parser = toml::Parser::new(&file_contents);
    let table = parser.parse().expect("Invalid toml");
    Ok(Config::deserialize(&mut toml::Decoder::new(toml::Value::Table(table))).expect("Config.toml was not of the expected format"))
}

fn main(){
    let cfg = load_config().expect("Couldn't load config");
    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let poll_duration = cfg.poll_duration;
    let timeout_duration = poll_duration*cfg.timeout_mul;

    let timer = timer();
    let client = TcpClient::new(AuroraProto)
        .connect(&cfg.tcp_address,&core.handle())
        .map_err(aurora::Error::from)
        .and_then(|client|{
            //let client = Timeout::new(client,Duration::from_secs(60));
            println!("Connected");
            let ev_stream = energy_voltage_stream(client,cfg.aurora_address);
            let ev_stream = RateLimitedStream::new(ev_stream,poll_duration,timer.clone(),2);

            timer.timeout_stream(ev_stream,timeout_duration)
            .map_err(aurora::Error::from)
            //Convert values to requests
            .map(move |(cum_e,cur_v)|{
                println!("{}Wh, {}V",cum_e,cur_v);
                let mut req = HttpRequest::new(Method::Post,"http://pvoutput.org/service/r2/addstatus.jsp".parse().expect("Hardcoded url is invalid?"));
                {
                    use mime::{Mime,TopLevel,SubLevel};
                    use hyper::header::*;

                    let headers = req.headers_mut();
                    headers.set_raw("X-Pvoutput-Apikey",cfg.pv_output.api_key.clone());
                    headers.set_raw("X-Pvoutput-SystemId",cfg.pv_output.system_id.clone());
                    headers.set(ContentType(Mime(TopLevel::Application,SubLevel::WwwFormUrlEncoded,vec![])));
                }
                let now = Local::now();
                let date = now.format("%Y%m%d");
                let time = now.format("%H:%M");
                let body = format!("d={}&t={}&v1={}&v6={}",date,time,cum_e,cur_v);
                println!("Body: {}",body);
                req.set_body(body);
                req
            })
            //upload stream
            .fold(hyper::Client::new(&handle),move |client, request|{
                println!("Uploading values");
                client.request(request)
                    .map_err(aurora::Error::from)
                    .and_then(move |res|{
                        if res.status() != &StatusCode::Ok{
                            write!(std::io::stderr(),"[WARNING]: Failed to upload status, continuing")
                        }
                        Ok(client)
                    })
            })
            .map(|_| ())
        });
    core.run(client).unwrap();
}
