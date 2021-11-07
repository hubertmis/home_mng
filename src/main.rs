use socket2::{Socket, Domain, Type};
use std::net::SocketAddr;
use std::sync::Arc;
use futures::{prelude::*,executor::LocalPool,task::LocalSpawnExt};
use async_coap::prelude::*;
use async_coap::datagram::{DatagramLocalEndpoint,AllowStdUdpSocket,DatagramRemoteEndpoint};
use async_coap::uri::{Uri, RelRef};

use async_coap::Error;
use async_coap::InboundContext;
use async_coap::message::{OwnedImmutableMessage, MessageWrite};

use std::time::{Duration, SystemTime};

use serde_cbor;
use std::collections::BTreeMap;
use futures::future::BoxFuture;

use clap::Parser;
use std::str::FromStr;

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

fn search_services(remote_endpoint: &DatagramRemoteEndpoint<AllowStdUdpSocket>) -> BoxFuture<Result<OwnedImmutableMessage, Error>> {
    fn service_finder(context: Result<&dyn InboundContext<SocketAddr = SocketAddr>, Error>,) -> Result<ResponseStatus<OwnedImmutableMessage>, Error> {
        let data : BTreeMap<String, BTreeMap<String, String>> = serde_cbor::from_slice(context.unwrap().message().payload()).unwrap();
        for (service, details) in data.iter() {
            println!("{}: {:?}: {}", service, details.get("type"), context.unwrap().remote_socket_addr());
        }

        Ok(ResponseStatus::Continue)
    }

    let future_result = remote_endpoint.send_to(
        rel_ref!("sd"),
        CoapRequest::get()
            .multicast()
            .use_handler(service_finder) 
        );

    future_result
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

fn post_data<'a, F>(remote_endpoint: &'a DatagramRemoteEndpoint<AllowStdUdpSocket>, path: &'a RelRef, writer: F) -> BoxFuture<'a, Result<OwnedImmutableMessage, Error>> 
    where
    F: 'a + Fn(&mut dyn MessageWrite) -> Result<(), Error> + Send + std::marker::Sync,
{
    let future_result = remote_endpoint.send_to(
        path,
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
    ServiceDiscovery,
    NotProvisioned,
    Provision(Prov),
    ResetProvisioning(ProvKey),
    Set(CoapSetter),
}

#[derive(Debug)]
enum ValType {
    StringType,
    IntType,
}

impl FromStr for ValType {
    type Err = clap::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "int" => Ok(ValType::IntType),
            "str" => Ok(ValType::StringType),
            _ => Err(clap::Error::with_description("Unknown error type. Use int or str".to_string(), clap::ErrorKind::InvalidValue)),
        }
    }
}

#[derive(Parser,Debug)]
struct Prov {
    #[clap(short, long)]
    addr: String,
    #[clap(short, long)]
    key: String,
    #[clap(short, long)]
    value: String,
    #[clap(short='t', default_value = "str")]
    value_type: ValType,
}

#[derive(Parser,Debug)]
struct ProvKey {
    #[clap(short, long)]
    addr: String,
    #[clap(short, long)]
    key: String,
}

#[derive(Parser,Debug)]
struct CoapSetter {
    #[clap(short, long)]
    addr: String,
    #[clap(short, long)]
    resource: String,
    #[clap(short, long)]
    key: String,
    #[clap(short, long)]
    value: String,
    #[clap(short='t', default_value = "str")]
    value_type: ValType,
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
        SubCommand::ServiceDiscovery => {
            let remote_endpoint = get_multicast_remote_endpoint(&local_endpoint);
            let future_result = search_services(&remote_endpoint);
            let result = pool.run_until(future_result);
        }

        SubCommand::NotProvisioned => {
            let remote_endpoint = get_multicast_remote_endpoint(&local_endpoint);
            let future_result = search_not_provisioned(&remote_endpoint);
            let result = pool.run_until(future_result);
        }

        SubCommand::Provision(prov) => {
            let remote_endpoint = get_addr_remote_endpoint(&local_endpoint, &prov.addr);
            let mut data_str = BTreeMap::new();
            let mut data_int = BTreeMap::new();
            let future_result;

            match prov.value_type {
                ValType::StringType => {
                    data_str.insert(prov.key, prov.value);

                    future_result = post_data(&remote_endpoint, rel_ref!("prov"), |msg_wrt| {
                        msg_wrt.set_msg_code(MsgCode::MethodPost);
                        serde_cbor::to_writer(msg_wrt, &data_str);
                        Ok(())
                    });
                }
                ValType::IntType => {
                    data_int.insert(prov.key, prov.value.parse::<i32>().unwrap());

                    future_result = post_data(&remote_endpoint, rel_ref!("prov"), |msg_wrt| {
                        msg_wrt.set_msg_code(MsgCode::MethodPost);
                        serde_cbor::to_writer(msg_wrt, &data_int);
                        Ok(())
                    });
                }
            }

            let result = pool.run_until(future_result);
        }

        SubCommand::ResetProvisioning(prov) => {
            let remote_endpoint = get_addr_remote_endpoint(&local_endpoint, &prov.addr);
            let mut data = BTreeMap::new();
            data.insert(prov.key, "");

            let future_result = post_data(&remote_endpoint, rel_ref!("prov"), |msg_wrt| {
                msg_wrt.set_msg_code(MsgCode::MethodPost);
                serde_cbor::to_writer(msg_wrt, &data);
                Ok(())
            });

            let result = pool.run_until(future_result);
        }

        SubCommand::Set(data) => {
            let remote_endpoint = get_addr_remote_endpoint(&local_endpoint, &data.addr);
            let mut data_str = BTreeMap::new();
            let mut data_int = BTreeMap::new();
            let future_result;

            match data.value_type {
                ValType::StringType => {
                    data_str.insert(data.key, data.value);

                    future_result = post_data(&remote_endpoint,
                                              RelRef::from_str(&data.resource).unwrap(),
                                              |msg_wrt| {
                        msg_wrt.set_msg_code(MsgCode::MethodPost);
                        serde_cbor::to_writer(msg_wrt, &data_str);
                        Ok(())
                    });
                }
                ValType::IntType => {
                    data_int.insert(data.key, data.value.parse::<i32>().unwrap());

                    future_result = post_data(&remote_endpoint,
                                              RelRef::from_str(&data.resource).unwrap(),
                                              |msg_wrt| {
                        msg_wrt.set_msg_code(MsgCode::MethodPost);
                        serde_cbor::to_writer(msg_wrt, &data_int);
                        Ok(())
                    });
                }
            }

            let result = pool.run_until(future_result);
        }
    }
}
