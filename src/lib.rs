use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use coap_lite::{CoapRequest, CoapResponse, ContentFormat, MessageClass, Packet, ResponseType};
use coap::client::{MessageReceiver, UdpCoAPClient};
use coap::request::{CoapOption, Method, RequestBuilder};

use socket2::{Socket, Domain, Type};

pub type Error = std::io::Error;

#[derive(Debug)]
pub enum Content {
    PlainText(String),
    Cbor(ciborium::value::Value),
}

impl Content {
    pub fn as_string(&self) -> Option<&String> {
        match self {
            Content::PlainText(string) => Some(string),
            _ => None,
        }
    }

    pub fn as_cbor(&self) -> Option<&ciborium::value::Value> {
        match self {
            Content::Cbor(cbor_value) => Some(cbor_value),
            _ => None,
        }
    }

    pub fn as_cbor_map(&self) -> Option<&Vec<(ciborium::value::Value, ciborium::value::Value)>> {
        self.as_cbor()?.as_map()
    }
}

pub struct Coap {
}

impl Coap {
    pub fn new() -> Self {
        Self {
        }
    }

    fn create_multicast_socket() -> std::net::UdpSocket {
        let udp_socket = Socket::new(Domain::IPV6, Type::DGRAM, None).expect("Socket creating failed");
        udp_socket.set_multicast_hops_v6(16).expect("Setting multicast hops failed");
        udp_socket.into()
    }

    fn set_content_format(request_builder: RequestBuilder, content_format: Option<ContentFormat>) -> RequestBuilder {
        if let Some(content_format) = content_format {
            request_builder.options([(CoapOption::ContentFormat,
                                      [usize::from(content_format) as u8].to_vec())
                                    ].to_vec())
        } else {
            request_builder
        }
    }

    async fn send_multicast(request: &CoapRequest<SocketAddr>) -> Result<(UdpCoAPClient, MessageReceiver), Error> {
        let port = 5683;
        let client = UdpCoAPClient::new_with_std_socket(Self::create_multicast_socket(), "[::1]:5683").await.unwrap();
        let receiver = client.create_receiver_for(&request).await;
        let peer_addr = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(
                0xff05, 0, 0, 0, 0, 0, 0, 0x1,
            )),
            port,
        );
        client.send_multicast(&request, &peer_addr).await.unwrap();

        Ok((client, receiver))
    }

    async fn post(addr: &SocketAddr, resource: &str, content_format: Option<ContentFormat>, payload: Option<Vec<u8>>) -> Result<CoapResponse, std::io::Error>{
        Self::post_con_or_non(addr, resource, content_format, payload, true).await
    }

    async fn post_non_confirmable(addr: &SocketAddr, resource: &str, content_format: Option<ContentFormat>, payload: Option<Vec<u8>>) -> Result<CoapResponse, std::io::Error>{
        Self::post_con_or_non(addr, resource, content_format, payload, false).await
    }

    async fn post_con_or_non(addr: &SocketAddr, resource: &str, content_format: Option<ContentFormat>, payload: Option<Vec<u8>>, con: bool) -> Result<CoapResponse, std::io::Error> {
        let domain = addr;
        let path = resource;

        let client = UdpCoAPClient::new_udp(addr).await?;
        let request = RequestBuilder::new(path, Method::Post)
            .domain(domain.to_string())
            .confirmable(con);
        let request = Self::set_content_format(request, content_format);

        let request = request
            .data(payload)
            .build();

        client.send(request).await
    }

    async fn post_provisioning(addr: &SocketAddr, tree: &ciborium::value::Value) -> Result<(), Error>
    {
        let mut payload = Vec::<u8>::new();
        ciborium::ser::into_writer(tree, &mut payload).expect("Could not serialize payload");
        let recv_packet = Self::post(addr, "prov", Some(ContentFormat::ApplicationCBOR), Some(payload)).await?;
        Self::response_type_is_expected(&recv_packet.message, &MessageClass::Response(ResponseType::Changed))
    }
        
    async fn init_search_services(srv_name: Option<&str>, srv_type: Option<&str>) -> Result<(UdpCoAPClient, MessageReceiver), Error> {
        let domain = "ff05::1";
        let path = "sd";

        let request = RequestBuilder::new(path, Method::Get)
            .domain(domain.to_string())
            .confirmable(false)
            .token(Some([0, 1, 2, 3].to_vec()));
        let request = if srv_name.is_some() || srv_type.is_some() {
            let mut data = BTreeMap::new();
            srv_name.and_then(|n| data.insert("name", n));
            srv_type.and_then(|t| data.insert("type", t));

            let mut payload = Vec::new();
            ciborium::ser::into_writer(&data, &mut payload).unwrap();

            Self::set_content_format(request, Some(ContentFormat::ApplicationCBOR))
                .data(Some(payload))
        } else {
            request
        };
        let request = request.build();

        Self::send_multicast(&request).await
    }

    async fn get_service(receiver: &mut MessageReceiver) -> Result<Option<coap::client::Packet>, Error> {
        if let Ok(recv_packet) = tokio::time::timeout(
            std::time::Duration::from_millis(2000),
            receiver.receive()).await {
                let recv_packet = recv_packet?;
                Self::response_type_is_expected(&recv_packet.message, &MessageClass::Response(ResponseType::Content))?;
                Ok(Some(recv_packet))
        } else {
            Ok(None)
        }
    }

    pub async fn service_discovery_single(&self, name: &str, srv_type: Option<&str>) -> Result<Option<(SocketAddr, Option<String>)>, Error> {
        let (_client, mut receiver) = Self::init_search_services(Some(name), srv_type).await?;
        while let Some(packet) = Self::get_service(&mut receiver).await? {
            let data: BTreeMap<String, BTreeMap<String, String>> = ciborium::de::from_reader(&packet.message.payload[..]).unwrap();
            let result = data.iter()
                .find_map(|(service, details)| if service == name && packet.address.is_some() {
                    Some((packet.address.unwrap(), details.get("type").map(|t| t.clone())))
                } else {
                    None
                });

            if result.is_some() {
                return Ok(result);
            }
        }
        Ok(None)
    }

    pub async fn service_discovery(&self, srv_name: Option<&str>, srv_type: Option<&str>) -> Result<Vec<(String, Option<String>, SocketAddr)>, Error> {
        let mut result = Vec::new();
        let (_client, mut receiver) = Self::init_search_services(srv_name, srv_type).await?;
        while let Some(packet) = Self::get_service(&mut receiver).await? {
            if let Some(address) = packet.address {
                // TODO: check content type
                let data: BTreeMap<String, BTreeMap<String, String>> = ciborium::de::from_reader(&packet.message.payload[..]).unwrap();
                for (service, details) in data.iter() {
                    result.push((service.clone(), details.get("type").map(|t| t.clone()), address));
                }
            }
        }
        Ok(result)
    }

    pub async fn not_provisioned_discovery(&self) -> Result<Vec<SocketAddr>, Error> {
        let mut result = Vec::new();
        let (_client, mut receiver) = Self::init_search_services(None, None).await?;
        while let Some(packet) = Self::get_service(&mut receiver).await? {
            if let Some(address) = packet.address {
                // TODO: check content type
                let data: BTreeMap<String, BTreeMap<String, String>> = ciborium::de::from_reader(&packet.message.payload[..]).unwrap();
                if data.len() == 0 {
                    result.push(address);
                }
            }
        }
        Ok(result)
    }

    pub async fn provision(&self, addr: &SocketAddr, payload_map: &ciborium::value::Value) -> Result<(), Error> {
        Self::post_provisioning(&addr, payload_map).await
    }

    pub async fn reset_provisioning(&self, addr: &SocketAddr, key: &str) -> Result<(), Error> {
        let key = ciborium::value::Value::Text(key.to_string());
        let reset_value = ciborium::value::Value::Text("".to_string());
        let tree = ciborium::value::Value::Map([(key, reset_value)].to_vec());

        Self::post_provisioning(&addr, &tree).await
    }

    pub async fn get(&self, addr: &SocketAddr, resource: &str, _payload_map: Option<&ciborium::value::Value>) -> Result<Option<Content>, Error> {
        let url = format!("coap://{}/{}", addr.to_string(), resource);
        let response = UdpCoAPClient::get(&url).await.unwrap();
        let payload = &response.message.payload;

        // TODO: insert payload

        for opt in response.message.options() {
            match (CoapOption::from(*opt.0) as CoapOption, opt.1) {
                (CoapOption::ContentFormat, cnt_fmt) => { 
                    for cnt_fmt in cnt_fmt {
                        match cnt_fmt[..] {
                            [] => {
                                let data = std::str::from_utf8(&payload).unwrap();
                                return Ok(Some(Content::PlainText(data.to_string())));
                            }
                            [60] => {
                                let data = ciborium::de::from_reader(&payload[..]).unwrap();
                                return Ok(Some(Content::Cbor(data)));
                            }
                            [unexpected_cf] =>
                                return Err(Error::new(std::io::ErrorKind::InvalidData,
                                                      format!("Unexpected content format: {}",
                                                              unexpected_cf))),
                            _ => return Err(Error::new(std::io::ErrorKind::InvalidData,
                                                       "Content format too long")),
                        }
                    }
                }
                _ => continue
            }
        }

        Ok(None)
    }

    pub async fn set(&self, addr: &SocketAddr, resource: &str, payload_map: &ciborium::value::Value) -> Result<(), Error> {
        let mut payload = Vec::<u8>::new();
        ciborium::ser::into_writer(&payload_map, &mut payload).expect("Could not serialize payload");
        let recv_packet = Self::post(addr, resource, Some(ContentFormat::ApplicationCBOR), Some(payload)).await?;
        Self::response_type_is_expected(&recv_packet.message, &MessageClass::Response(ResponseType::Changed))
    }

    pub async fn set_non_confirmable(&self, addr: &SocketAddr, resource: &str, payload_map: &ciborium::value::Value) -> Result<(), Error> {
        let mut payload = Vec::<u8>::new();
        ciborium::ser::into_writer(&payload_map, &mut payload).expect("Could not serialize payload");
        Self::post_non_confirmable(addr, resource, Some(ContentFormat::ApplicationCBOR), Some(payload)).await?;
        Ok(())
    }

    pub async fn fota_req(&self, rmt_addr: &SocketAddr, local_addr: &str) -> Result<(), Error> {
        let payload = format!("coap://{}/fota", local_addr);
        let recv_packet = Self::post(rmt_addr, "fota_req", Some(ContentFormat::TextPlain), Some(payload.into())).await?;
        Self::response_type_is_expected(&recv_packet.message, &MessageClass::Response(ResponseType::Changed))
    }

    fn response_type_is_expected(recv_packet: &Packet, expected_msg_class: &MessageClass) -> Result<(), Error> {
        if &recv_packet.header.code == expected_msg_class {
            Ok(())
        } else {
            Err(Error::new(std::io::ErrorKind::InvalidData,
                           format!("Expected {} reponse type but received {}",
                                   expected_msg_class,
                                   recv_packet.header.get_code())))
        }
    }
}
