use socket2::{Socket, Domain, Type};
use std::net::SocketAddr;
use std::sync::Arc;
use futures::{prelude::*,executor::LocalPool,task::LocalSpawnExt};
use async_coap::prelude::*;
use async_coap::datagram::{DatagramLocalEndpoint,AllowStdUdpSocket,DatagramRemoteEndpoint};
use async_coap::uri::Uri;

use async_coap::Error;
use async_coap::InboundContext;
use async_coap::message::{OwnedImmutableMessage, MessageWrite};

use std::time::{Duration, SystemTime};

use serde_cbor;
use std::collections::BTreeMap;
use futures::future::BoxFuture;

use clap::Parser;

fn get_remote_endpoint(local_endpoint: &Arc<DatagramLocalEndpoint<AllowStdUdpSocket>>, addr: &Uri) -> DatagramRemoteEndpoint<AllowStdUdpSocket> {
    local_endpoint
        .remote_endpoint_from_uri(addr)
        .unwrap()
}

fn get_multicast_remote_endpoint(local_endpoint: &Arc<DatagramLocalEndpoint<AllowStdUdpSocket>>) -> DatagramRemoteEndpoint<AllowStdUdpSocket> {
    get_remote_endpoint(local_endpoint, uri!("coap://[ff05::1]"))
}

fn get_addr_remote_endpoint(local_endpoint: &Arc<DatagramLocalEndpoint<AllowStdUdpSocket>>, addr: &str) -> DatagramRemoteEndpoint<AllowStdUdpSocket> {
    let uri = String::new() + "coap://[" + addr + "]";
    get_remote_endpoint(&local_endpoint, Uri::from_str(&uri).unwrap())
}

fn search_not_provisioned(remote_endpoint: &DatagramRemoteEndpoint<AllowStdUdpSocket>) -> BoxFuture<Result<OwnedImmutableMessage, Error>> {
    fn not_prov_finder(context: Result<&dyn InboundContext<SocketAddr = SocketAddr>, Error>,) -> Result<ResponseStatus<OwnedImmutableMessage>, Error> {
        let data : BTreeMap<String, BTreeMap<String, String>> = serde_cbor::from_slice(context.unwrap().message().payload()).unwrap();
        if data.len() == 0 {
            println!("Addr: {}", context.unwrap().remote_socket_addr());
        }

        Ok(ResponseStatus::Continue)
    }

    let future_result = remote_endpoint.send_to(
        rel_ref!("sd"),
        CoapRequest::get()
            .multicast()
            .use_handler(not_prov_finder) 
        );

    future_result
}

fn post_data<'a, F>(remote_endpoint: &'a DatagramRemoteEndpoint<AllowStdUdpSocket>, writer: F) -> BoxFuture<'a, Result<OwnedImmutableMessage, Error>> 
    where
    F: 'a + Fn(&mut dyn MessageWrite) -> Result<(), Error> + Send + std::marker::Sync,
{
    let future_result = remote_endpoint.send_to(
        rel_ref!("prov"),
        CoapRequest::post()
            .content_format(ContentFormat::APPLICATION_CBOR)
            .payload_writer(writer)
            .emit_successful_response()
        );

    future_result
}

#[derive(Parser,Debug)]
#[clap(version = "0.1", author = "Hubert Miś <hubert.mis@gmail.com>")]
struct Opts {
    #[clap(subcommand)]
    subcmd: SubCommand,
}

#[derive(Parser,Debug)]
enum SubCommand {
    NotProvisioned,
    Provision(Prov),
    ResetProvisioning(ProvKey),
}

enum ProvVal {
    String(String),
    Int(i32),
}

#[derive(Parser,Debug)]
struct Prov {
    #[clap(short, long)]
    addr: String,
    #[clap(short, long)]
    key: String,
    #[clap(short, long)]
    value: String,
}

#[derive(Parser,Debug)]
struct ProvKey {
    #[clap(short, long)]
    addr: String,
    #[clap(short, long)]
    key: String,
}

fn main() {
    let opts = Opts::parse();
    println!("{:?}", opts);

    let udp_socket = Socket::new(Domain::IPV6, Type::DGRAM, None).expect("Socket creating failed");
    let address: SocketAddr = "[::]:0".parse().unwrap();
    let address = address.into();
    udp_socket.set_nonblocking(true).unwrap();
    udp_socket.set_multicast_hops_v6(16).expect("Setting multicast hops failed");
    udp_socket.bind(&address).expect("UDP bind failed");

    let socket = AllowStdUdpSocket::from_std(udp_socket.into());

    let local_endpoint = Arc::new(DatagramLocalEndpoint::new(socket));

    let mut pool = LocalPool::new();
    pool.spawner().spawn_local(
        local_endpoint
        .clone()
        .receive_loop_arc(null_receiver!())
        .map(|err| panic!("Receive loop terminated: {}", err))
        );

    match opts.subcmd {
        SubCommand::NotProvisioned => {
            let remote_endpoint = get_multicast_remote_endpoint(&local_endpoint);
            let future_result = search_not_provisioned(&remote_endpoint);
            let result = pool.run_until(future_result);
        }
        SubCommand::Provision(prov) => {
            let remote_endpoint = get_addr_remote_endpoint(&local_endpoint, &prov.addr);
            let mut data = BTreeMap::new();
            data.insert(prov.key, prov.value);

            let future_result = post_data(&remote_endpoint, |msg_wrt| {
                msg_wrt.set_msg_code(MsgCode::MethodPost);
                serde_cbor::to_writer(msg_wrt, &data);
                Ok(())
            });

            let result = pool.run_until(future_result);
        }
        SubCommand::ResetProvisioning(prov) => {
            let remote_endpoint = get_addr_remote_endpoint(&local_endpoint, &prov.addr);
            let mut data = BTreeMap::new();
            data.insert(prov.key, "");

            let future_result = post_data(&remote_endpoint, |msg_wrt| {
                msg_wrt.set_msg_code(MsgCode::MethodPost);
                serde_cbor::to_writer(msg_wrt, &data);
                Ok(())
            });

            let result = pool.run_until(future_result);
        }
    }
}
