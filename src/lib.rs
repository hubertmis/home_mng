use std::collections::BTreeMap;
use std::fmt::Write;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use async_coap::prelude::*;
use async_coap::Error;
use async_coap::InboundContext;
use async_coap::{LocalEndpoint as _AsyncCoapLocalEndpoint, RemoteEndpoint as _AsyncCoapRemoteEndpoint};
use async_coap::datagram::{DatagramLocalEndpoint,AllowStdUdpSocket,DatagramRemoteEndpoint};
use async_coap::message::{OwnedImmutableMessage, MessageWrite};
use async_coap::uri::{Uri, RelRef};

use futures::prelude::*;

use socket2::{Socket, Domain, Type};

pub struct Value (
    ciborium::value::Value,
);

impl FromStr for Value {
    type Err = clap::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Value(ciborium::value::Value::Text(s.to_string())))
    }
}

impl Value {
    pub fn from_type_and_str(t: &str, s: &str) -> Result<Self, Box<dyn std::error::Error>> {
        match t {
            "int" => Ok(Value(ciborium::value::Value::Integer(s.parse::<i32>()?.try_into()?))),
            "str" => Ok(Value(ciborium::value::Value::Text(s.to_string()))),
            "bin" => Ok(Value(ciborium::value::Value::Bytes(Vec::new()))), // TODO! .chunks(), <u8>.from_str_radix??
            "bool" => Ok(Value(ciborium::value::Value::Bool(s.parse::<bool>()?))),
            _ => Err("Invalid value's type".into()),
        }
    }
}

type LocalEndpoint = DatagramLocalEndpoint<AllowStdUdpSocket>;
type RemoteEndpoint = DatagramRemoteEndpoint<AllowStdUdpSocket>;

pub struct Coap {
    pub local_endpoint: LocalEndpoint,
}

impl Coap {
    pub fn new() -> Self {
        let udp_socket = Socket::new(Domain::IPV6, Type::DGRAM, None).expect("Socket creating failed");
        let address: SocketAddr = "[::]:0".parse().unwrap();
        let address = address.into();
        udp_socket.set_nonblocking(true).unwrap();
        udp_socket.set_multicast_hops_v6(16).expect("Setting multicast hops failed");
        udp_socket.bind(&address).expect("UDP bind failed");

        let socket = AllowStdUdpSocket::from_std(udp_socket.into());

        Self {
            local_endpoint: DatagramLocalEndpoint::new(socket),
        }
    }

    pub async fn receive_loop_arc(self: Arc<Self>) {
        self.local_endpoint
            .receive_loop(null_receiver!())
            .map(|err| panic!("Receive loop terminated: {}", err))
            .await
    }

    fn get_remote_endpoint(&self, addr: &Uri) -> RemoteEndpoint {
        self.local_endpoint
            .remote_endpoint_from_uri(addr)
            .unwrap()
    }

    fn get_addr_remote_endpoint(&self, addr: &str) -> RemoteEndpoint {
        let uri = String::new() + "coap://[" + addr + "]";
        self.get_remote_endpoint(Uri::from_str(&uri).unwrap())
    }

    fn get_multicast_remote_endpoint(&self) -> RemoteEndpoint {
        self.get_remote_endpoint(uri!("coap://[ff05::1]"))
    }

    async fn post_data<F>(remote_endpoint: &RemoteEndpoint, path: &RelRef, writer: F) -> Result<OwnedImmutableMessage, Error>
        where
        F: Fn(&mut dyn MessageWrite) -> Result<(), Error> + Send + std::marker::Sync,
    {
        remote_endpoint.send_to(
            path,
            CoapRequest::post()
                .content_format(ContentFormat::APPLICATION_CBOR)
                .payload_writer(writer)
                .emit_successful_response()
            )
        .await
    }

    async fn post_provisioning(remote_endpoint: &RemoteEndpoint, tree: &BTreeMap<&str, &ciborium::value::Value>) -> Result<OwnedImmutableMessage, Error>
    {
        Self::post_data(&remote_endpoint, rel_ref!("prov"), |msg_wrt| {
            msg_wrt.set_msg_code(MsgCode::MethodPost);
            ciborium::ser::into_writer(&tree, msg_wrt).unwrap();
            Ok(())
        })
        .await
    }
        
    async fn search_services<'a: 'd, 'b: 'd, 'c: 'd, 'd>(remote_endpoint: &'a RemoteEndpoint,
                       srv_name: Option<&'b str>, srv_type: Option<&'c str>) -> Result<OwnedImmutableMessage, Error> {
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
                .await
        } else {
            remote_endpoint.send_to(
                rel_ref!("sd"),
                CoapRequest::get()
                    .multicast()
                    .use_handler(service_finder)
                )
                .await
        }
    }

    async fn search_not_provisioned(remote_endpoint: &RemoteEndpoint) -> Result<OwnedImmutableMessage, Error> {
        fn not_prov_finder(context: Result<&dyn InboundContext<SocketAddr = SocketAddr>, Error>,) -> Result<ResponseStatus<OwnedImmutableMessage>, Error> {
            let data : BTreeMap<String, BTreeMap<String, String>> = ciborium::de::from_reader(context.unwrap().message().payload()).unwrap();
            if data.len() == 0 {
                println!("Addr: {}", context.unwrap().remote_socket_addr());
            }

            Ok(ResponseStatus::Continue)
        }

        remote_endpoint.send_to(
            rel_ref!("sd"),
            CoapRequest::get()
                .multicast()
                .use_handler(not_prov_finder) 
            )
            .await
    }

    pub async fn service_discovery(&self, srv_name: Option<&str>, srv_type: Option<&str>) -> Result<OwnedImmutableMessage, Error> {
        let remote_endpoint = self.get_multicast_remote_endpoint();
        Self::search_services(&remote_endpoint, srv_name, srv_type).await
    }

    pub async fn not_provisioned_discovery(&self) -> Result<OwnedImmutableMessage, Error> {
        let remote_endpoint = self.get_multicast_remote_endpoint();
        Self::search_not_provisioned(&remote_endpoint).await
    }

    pub async fn provision(&self, addr: &str, key: &str, value: &Value) -> Result<OwnedImmutableMessage, Error> {
        let remote_endpoint = self.get_addr_remote_endpoint(addr);
        let tree = BTreeMap::from([
            (key, &value.0),
        ]);

        Self::post_provisioning(&remote_endpoint, &tree).await
    }

    pub async fn reset_provisioning(&self, addr: &str, key: &str) -> Result<OwnedImmutableMessage, Error> {
        let remote_endpoint = self.get_addr_remote_endpoint(addr);
        let reset_value = ciborium::value::Value::Text("".to_string());
        let tree = BTreeMap::from([
            (key, &reset_value),
        ]);

        Self::post_provisioning(&remote_endpoint, &tree).await
    }

    pub async fn get(&self, addr: &str, resource: &str, payload_map: Option<&ciborium::value::Value>) -> Result<(), Error> {
        let remote_endpoint = self.get_addr_remote_endpoint(addr);
        let uri = RelRef::from_str(resource).unwrap();
        remote_endpoint.send_to(
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
                ).await
    }

    pub async fn set(&self, addr: &str, resource: &str, payload_map: &ciborium::value::Value) -> Result<OwnedImmutableMessage, Error> {
        let remote_endpoint = self.get_addr_remote_endpoint(addr);
        Self::post_data(&remote_endpoint,
                  RelRef::from_str(resource).unwrap(),
                  |msg_wrt| {
            msg_wrt.set_msg_code(MsgCode::MethodPost);
            ciborium::ser::into_writer(&payload_map, msg_wrt).unwrap();
            Ok(())
        }).await
    }

    pub async fn fota_req(&self, rmt_addr: &str, local_addr: &str) -> Result<OwnedImmutableMessage, Error> {
        let payload = format!("coap://{}/fota", local_addr);
        let remote_endpoint = self.get_addr_remote_endpoint(rmt_addr);

        remote_endpoint.send_to(
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
            ).await
    }
}
