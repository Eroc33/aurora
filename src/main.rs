extern crate aurora;
extern crate futures;
extern crate tokio_core;
extern crate tokio_service;
extern crate tokio_proto;

use futures::Future;
use tokio_core::reactor::Core;
use tokio_service::Service;
use tokio_proto::TcpClient;
use aurora::{AuroraProto,Request,Response};

fn main(){
    let sock_addr = "192.168.1.100:100".parse().unwrap();
    let mut core = Core::new().unwrap();
    let client = TcpClient::new(AuroraProto)
       .connect(&sock_addr,&core.handle())
       .and_then(|client|{
           println!("Connected");
           client.call((2,Request::State)).map(|ver|{
               println!("{:?}",ver);
           })
       });
    core.run(client).unwrap();
}
