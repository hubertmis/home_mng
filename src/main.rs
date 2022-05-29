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

use std::collections::BTreeMap;
use futures::future::BoxFuture;

use clap::Parser;
use std::str::FromStr;

use std::fmt::Write;

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

fn search_services(remote_endpoint: &DatagramRemoteEndpoint<AllowStdUdpSocket>,
                   srv_name: Option<String>, srv_type: Option<String>) -> BoxFuture<Result<OwnedImmutableMessage, Error>> {
    fn service_finder(context: Result<&dyn InboundContext<SocketAddr = SocketAddr>, Error>,) -> Result<ResponseStatus<OwnedImmutableMessage>, Error> {
        let data : BTreeMap<String, BTreeMap<String, String>> = ciborium::de::from_reader(context.unwrap().message().payload()).unwrap();
        for (service, details) in data.iter() {
            println!("{}: {:?}: {}", service, details.get("type"), context.unwrap().remote_socket_addr());
        }

        Ok(ResponseStatus::Continue)
    }

    if srv_name.is_some() || srv_type.is_some() {
        remote_endpoint.send_to(
            rel_ref!("sd"),
            CoapRequest::get()
                .multicast()
                .content_format(ContentFormat::APPLICATION_CBOR)
                .payload_writer(move |msg_wrt| {
                            let mut data = BTreeMap::new();
                            if let Some(srv_name) = &srv_name {
                                data.insert("name", srv_name);
                            }
                            if let Some(srv_type) = &srv_type {
                                data.insert("type", srv_type);
                            }
                            msg_wrt.set_msg_type(MsgType::Non);
                            msg_wrt.set_msg_code(MsgCode::MethodGet);
                            ciborium::ser::into_writer(&data, msg_wrt).unwrap();
                            Ok(())
                })
                .use_handler(service_finder)
            )
    } else {
        remote_endpoint.send_to(
            rel_ref!("sd"),
            CoapRequest::get()
                .multicast()
                .use_handler(service_finder)
            )
    }
}

fn search_not_provisioned(remote_endpoint: &DatagramRemoteEndpoint<AllowStdUdpSocket>) -> BoxFuture<Result<OwnedImmutableMessage, Error>> {
    fn not_prov_finder(context: Result<&dyn InboundContext<SocketAddr = SocketAddr>, Error>,) -> Result<ResponseStatus<OwnedImmutableMessage>, Error> {
        let data : BTreeMap<String, BTreeMap<String, String>> = ciborium::de::from_reader(context.unwrap().message().payload()).unwrap();
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
    ServiceDiscovery(SdFilter),
    NotProvisioned,
    Provision(Prov),
    ResetProvisioning(ProvKey),
    Get(CoapGetter),
    Set(CoapSetter),
    FotaReq(CoapFotaReq),
}

#[derive(Debug)]
enum ValType {
    StringType,
    IntType,
    BinType,
    BoolType,
}

impl FromStr for ValType {
    type Err = clap::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "int" => Ok(ValType::IntType),
            "str" => Ok(ValType::StringType),
            "bin" => Ok(ValType::BinType),
            "bool" => Ok(ValType::BoolType),
            _ => Err(clap::Error::raw(clap::ErrorKind::InvalidValue, "Unknown value type. Use int, str, or bin")),
        }
    }
}

#[derive(Parser,Debug)]
struct SdFilter {
    #[clap(short = 'n', long)]
    service_name: Option<String>,
    #[clap(short = 't', long)]
    service_type: Option<String>,
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
struct CoapGetter {
    #[clap(short, long)]
    addr: String,
    #[clap(short, long)]
    resource: String,
    #[clap(short, long)]
    key: Option<String>,
    #[clap(short, long)]
    value: Option<String>,
    #[clap(short='t', default_value = "str")]
    value_type: ValType,
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
    #[clap(long)]
    keys: Option<String>,
    #[clap(long)]
    values: Option<String>,
    #[clap(short='e')]
    exp_rsp: bool,
}

#[derive(Parser,Debug)]
struct CoapFotaReq {
    #[clap(short, long)]
    addr: String,
}

fn encode_req_payload(key: String, value: String, keys:Option<String>, values: Option<String>, value_type: ValType) -> BTreeMap<String, ciborium::value::Value> {
    let mut data_map: BTreeMap<String, ciborium::value::Value> = BTreeMap::new();

    match value_type {
        ValType::StringType => {
            data_map.insert(key, ciborium::value::Value::Text(value));
        }
        ValType::IntType => {
            if let (Some(keys), Some(values)) = (keys, values) {
                let key_iter = keys.split(',');
                let value_iter = values.split(',');
                for (k, v) in key_iter.zip(value_iter) {
                    data_map.insert(k.to_string(), ciborium::value::Value::Integer(
                            ciborium::value::Integer::from(
                                v.parse::<i32>().unwrap()
                                )
                            ));
                }
            } else {
                data_map.insert(key, ciborium::value::Value::Integer(
                        ciborium::value::Integer::from(
                            value.parse::<i32>().unwrap()
                            )
                        ));
            }
        }
        ValType::BinType => {
            let bin_vec = value
                .chars()
                .collect::<Vec<char>>()
                .chunks(2)
                .map(|c| c.iter().collect::<String>())
                .map(|c| u8::from_str_radix(&c, 16).unwrap())
                .collect::<Vec<u8>>();

            data_map.insert(key, ciborium::value::Value::Bytes(bin_vec));
        }
        ValType::BoolType => {
            data_map.insert(key, ciborium::value::Value::Bool(
                        value.parse::<bool>().unwrap()
                    ));
        }
    }

    data_map
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
        )
        .unwrap();

    match opts.subcmd {
        SubCommand::ServiceDiscovery(sd_filter) => {
            let remote_endpoint = get_multicast_remote_endpoint(&local_endpoint);
            let future_result = search_services(&remote_endpoint, sd_filter.service_name, sd_filter.service_type);
            let _result = pool.run_until(future_result);
        }

        SubCommand::NotProvisioned => {
            let remote_endpoint = get_multicast_remote_endpoint(&local_endpoint);
            let future_result = search_not_provisioned(&remote_endpoint);
            let _result = pool.run_until(future_result);
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
                        ciborium::ser::into_writer(&data_str, msg_wrt).unwrap();
                        Ok(())
                    });
                }
                ValType::IntType => {
                    data_int.insert(prov.key, prov.value.parse::<i32>().unwrap());

                    future_result = post_data(&remote_endpoint, rel_ref!("prov"), |msg_wrt| {
                        msg_wrt.set_msg_code(MsgCode::MethodPost);
                        ciborium::ser::into_writer(&data_int, msg_wrt).unwrap();
                        Ok(())
                    });
                }
                ValType::BinType => {
                    return;
                }
                ValType::BoolType => {
                    return;
                }
            }

            let _result = pool.run_until(future_result);
        }

        SubCommand::ResetProvisioning(prov) => {
            let remote_endpoint = get_addr_remote_endpoint(&local_endpoint, &prov.addr);
            let mut data = BTreeMap::new();
            data.insert(prov.key, "");

            let future_result = post_data(&remote_endpoint, rel_ref!("prov"), |msg_wrt| {
                msg_wrt.set_msg_code(MsgCode::MethodPost);
                ciborium::ser::into_writer(&data, msg_wrt).unwrap();
                Ok(())
            });

            let _result = pool.run_until(future_result);
        }

        SubCommand::Get(data) => {
            let payload_map;

            if let (Some(key), Some(val)) = (data.key, data.value) {
                payload_map = Some(encode_req_payload(key, val, None, None, data.value_type));
            } else {
                payload_map = None;
            }

            let remote_endpoint = get_addr_remote_endpoint(&local_endpoint, &data.addr);
            let uri = RelRef::from_str(&data.resource).unwrap();
            let future_result = remote_endpoint.send_to(
                uri,
                CoapRequest::get()
                    .payload_writer(|msg_wrt| {
                            msg_wrt.clear();
                            msg_wrt.set_msg_type(MsgType::Con);
                            msg_wrt.set_msg_code(MsgCode::MethodGet);
                            msg_wrt.set_msg_token(MsgToken::EMPTY);
                            for path_item in uri.path_segments() {
                                msg_wrt.insert_option_with_str(OptionNumber::URI_PATH, &path_item).unwrap();
                            }
                            if let Some(payload_map) = &payload_map {
                                msg_wrt.insert_option_with_u32(OptionNumber::CONTENT_FORMAT, ContentFormat::APPLICATION_CBOR.0.into()).unwrap();
                                ciborium::ser::into_writer(&payload_map, msg_wrt).unwrap();
                            }
                            Ok(())
                        })
                    .use_handler(|context| {
                            let msg_read = context.unwrap().message();

                            for opt in msg_read.options() {
                                match opt {
                                    Ok((async_coap::option::OptionNumber::CONTENT_FORMAT, cnt_fmt)) => {
                                        match cnt_fmt {
                                            [] => {
                                                let data = std::str::from_utf8(msg_read.payload()).unwrap();
                                                println!("Data: {}", data);
                                            }
                                            [60] => {
                                                let data : ciborium::value::Value = ciborium::de::from_reader(msg_read.payload()).unwrap();
                                                println!("Data: {:?}", data);
                                            }
                                            _ => println!("Unknown content format"),
                                        }
                                    }
                                    _ => {}
                                }
                            }

                            Ok(ResponseStatus::Done(()))
                        })
                    );

            let _result = pool.run_until(future_result);
        }

        SubCommand::Set(data) => {
            let remote_endpoint = get_addr_remote_endpoint(&local_endpoint, &data.addr);
            let data_map = encode_req_payload(data.key, data.value, data.keys, data.values, data.value_type);

            let future_result = post_data(&remote_endpoint,
                                          RelRef::from_str(&data.resource).unwrap(),
                                          |msg_wrt| {
                msg_wrt.set_msg_code(MsgCode::MethodPost);
                ciborium::ser::into_writer(&data_map, msg_wrt).unwrap();
                Ok(())
            });

            let _result = pool.run_until(future_result);
        }

        SubCommand::FotaReq(data) => {
            let mut local_addr_opt = None;

            for iface in pnet::datalink::interfaces() {
                for ip in iface.ips {
                    if let ipnetwork::IpNetwork::V6(network) = ip {
                        if network.prefix() < 128 && (network.ip().octets()[0] & 0xe0 == 0x20) {
                            local_addr_opt = Some(network.ip());
                            break;
                        }
                    }
                }
            }

            let local_addr = local_addr_opt.expect("Local IPv6 address not found");
            let payload = format!("coap://[{}]/fota", local_addr);

            let remote_endpoint = get_addr_remote_endpoint(&local_endpoint, &data.addr);
            let future_result = remote_endpoint.send_to(
                RelRef::from_str("fota_req").unwrap(),
                CoapRequest::post()
                    .content_format(ContentFormat::TEXT_PLAIN_UTF8)
                    .payload_writer(|msg_wrt| { 
                        msg_wrt.clear();
                        msg_wrt.set_msg_type(MsgType::Con);
                        msg_wrt.set_msg_code(MsgCode::MethodPost);
                        msg_wrt.set_msg_token(MsgToken::EMPTY);
                        msg_wrt.insert_option_with_str(OptionNumber::URI_PATH, "fota_req").unwrap();
                        msg_wrt.write_str(&payload).unwrap();
                        Ok(())
                    })
                    .emit_successful_response()
                );

            let _result = pool.run_until(future_result);
        }
    }
}
