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
                            ciborium::ser::into_writer(&data, msg_wrt);
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
    RgbwType,
}

impl FromStr for ValType {
    type Err = clap::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "int" => Ok(ValType::IntType),
            "str" => Ok(ValType::StringType),
            "bin" => Ok(ValType::BinType),
            "rgbw" => Ok(ValType::RgbwType),
            _ => Err(clap::Error::with_description("Unknown error type. Use int or str".to_string(), clap::ErrorKind::InvalidValue)),
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
    #[clap(short='e')]
    exp_rsp: bool,
}

#[derive(Parser,Debug)]
struct CoapFotaReq {
    #[clap(short, long)]
    addr: String,
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
        SubCommand::ServiceDiscovery(sd_filter) => {
            let remote_endpoint = get_multicast_remote_endpoint(&local_endpoint);
            let future_result = search_services(&remote_endpoint, sd_filter.service_name, sd_filter.service_type);
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
                        ciborium::ser::into_writer(&data_str, msg_wrt);
                        Ok(())
                    });
                }
                ValType::IntType => {
                    data_int.insert(prov.key, prov.value.parse::<i32>().unwrap());

                    future_result = post_data(&remote_endpoint, rel_ref!("prov"), |msg_wrt| {
                        msg_wrt.set_msg_code(MsgCode::MethodPost);
                        ciborium::ser::into_writer(&data_int, msg_wrt);
                        Ok(())
                    });
                }
                ValType::BinType => {
                    return;
                }
                ValType::RgbwType => {
                    return;
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
                ciborium::ser::into_writer(&data, msg_wrt);
                Ok(())
            });

            let result = pool.run_until(future_result);
        }

        SubCommand::Get(data) => {
            let remote_endpoint = get_addr_remote_endpoint(&local_endpoint, &data.addr);
            let future_result = remote_endpoint.send_to(
                RelRef::from_str(&data.resource).unwrap(),
                CoapRequest::get()
                    .payload_writer(|msg_wrt| {
                        msg_wrt.clear();
                        msg_wrt.set_msg_type(MsgType::Con);
                        msg_wrt.set_msg_code(MsgCode::MethodGet);
                        msg_wrt.set_msg_token(MsgToken::EMPTY);
                        msg_wrt.insert_option_with_str(OptionNumber::URI_PATH, &data.resource);
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

            let result = pool.run_until(future_result);
        }

        SubCommand::Set(data) => {
            let remote_endpoint = get_addr_remote_endpoint(&local_endpoint, &data.addr);
            let mut data_map: BTreeMap<&str, ciborium::value::Value> = BTreeMap::new();
            let mut bin_vec: Vec<u8> = Vec::new();

            if data.exp_rsp {
                data_map.insert("ersp", ciborium::value::Value::Bool(true));
            }

            match data.value_type {
                ValType::StringType => {
                    data_map.insert(&data.key, ciborium::value::Value::Text(data.value));

                }
                ValType::IntType => {
                    data_map.insert(&data.key, ciborium::value::Value::Integer(
                            ciborium::value::Integer::from(
                                data.value.parse::<i32>().unwrap()
                                )
                            ));
                }
                ValType::BinType => {
                    bin_vec = data.value
                        .chars()
                        .collect::<Vec<char>>()
                        .chunks(2)
                        .map(|c| c.iter().collect::<String>())
                        .map(|c| u8::from_str_radix(&c, 16).unwrap())
                        .collect::<Vec<u8>>();

                    data_map.insert(&data.key, ciborium::value::Value::Bytes(bin_vec));
                }
                ValType::RgbwType => {
                    #[derive(Debug)]
                    struct RgbwValues {
                        r: u16,
                        g: u16,
                        b: u16,
                        w: u16,
                        d: u16,
                    };
                    impl std::str::FromStr for RgbwValues {
                        type Err = String;
                        fn from_str(value: &str) -> Result<Self, Self::Err> {
                            if value.len() != 10 { return Err("Invalid string length".to_string()); }

                            let val = value
                                .chars()
                                .collect::<Vec<char>>()
                                .chunks(2)
                                .map(|c| c.iter().collect::<String>())
                                .collect::<Vec<String>>();
                            Ok(RgbwValues {
                                r: u16::from_str_radix(&val[0], 16).unwrap(),
                                g: u16::from_str_radix(&val[1], 16).unwrap(),
                                b: u16::from_str_radix(&val[2], 16).unwrap(),
                                w: u16::from_str_radix(&val[3], 16).unwrap(),
                                d: u16::from_str_radix(&val[4], 16).unwrap(),
                            })
                        }
                    }
                    let input: RgbwValues = data.value.parse().unwrap();
                    data_map.insert("r", ciborium::value::Value::Integer(ciborium::value::Integer::from(input.r)));
                    data_map.insert("g", ciborium::value::Value::Integer(ciborium::value::Integer::from(input.g)));
                    data_map.insert("b", ciborium::value::Value::Integer(ciborium::value::Integer::from(input.b)));
                    data_map.insert("w", ciborium::value::Value::Integer(ciborium::value::Integer::from(input.w)));
                    data_map.insert("d", ciborium::value::Value::Integer(ciborium::value::Integer::from(input.d * 100)));
                }
            }

            let future_result = post_data(&remote_endpoint,
                                          RelRef::from_str(&data.resource).unwrap(),
                                          |msg_wrt| {
                msg_wrt.set_msg_code(MsgCode::MethodPost);
                ciborium::ser::into_writer(&data_map, msg_wrt);
                Ok(())
            });

            let result = pool.run_until(future_result);
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
                        msg_wrt.insert_option_with_str(OptionNumber::URI_PATH, "fota_req");
                        msg_wrt.write_str(&payload).unwrap();
                        Ok(())
                    })
                    .emit_successful_response()
                );

            let result = pool.run_until(future_result);
        }
    }
}
