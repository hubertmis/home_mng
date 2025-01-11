use clap::{Args, Parser, Subcommand};
use std::io::BufReader;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

#[derive(Parser, Debug)]
#[clap(version = "0.1", author = "Hubert Miś <hubert.mis@gmail.com>")]
struct Opts {
    #[clap(subcommand)]
    subcmd: SubCommand,
}

#[derive(Subcommand, Debug)]
enum SubCommand {
    ServiceDiscovery(SdFilter),
    NotProvisioned,
    Provision(Prov),
    ResetProvisioning(ProvKey),
    Get(CoapGetter),
    Set(CoapSetter),
    FotaReq(CoapFotaReq),
}

#[derive(Clone, Debug)]
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
            _ => Err(clap::Error::raw(
                clap::error::ErrorKind::InvalidValue,
                "Unknown value type. Use int, str, or bin",
            )),
        }
    }
}

impl ToString for ValType {
    fn to_string(&self) -> String {
        match self {
            ValType::StringType => "str",
            ValType::IntType => "int",
            ValType::BinType => "bin",
            ValType::BoolType => "bool",
        }
        .to_string()
    }
}

#[derive(Args, Debug)]
struct SdFilter {
    #[clap(short = 'n', long)]
    service_name: Option<String>,
    #[clap(short = 't', long)]
    service_type: Option<String>,
    #[clap(short, long)]
    single: bool,
}

#[derive(Args, Debug)]
struct Prov {
    #[clap(short, long)]
    addr: String,
    #[command(flatten)]
    req_payload: RequestPayload,
}

#[derive(Args, Debug)]
struct ProvKey {
    #[clap(short, long)]
    addr: String,
    #[clap(short, long)]
    key: String,
}

#[derive(Args, Debug)]
struct CoapGetter {
    #[clap(short, long)]
    addr: String,
    #[clap(short, long)]
    resource: String,
    #[command(flatten)]
    req_payload: RequestPayload,
}

#[derive(Args, Debug)]
struct CoapSetter {
    #[clap(short, long)]
    addr: String,
    #[clap(short, long)]
    resource: String,
    #[clap(flatten)]
    req_payload: RequestPayload,
    #[clap(short = 'e')]
    exp_rsp: bool,
    #[clap(long)]
    non_confirmable: bool,
}

#[derive(Args, Debug)]
struct RequestPayload {
    #[arg(short, long, requires = "value")]
    key: Option<String>,
    #[arg(short, long, requires = "key")]
    value: Option<String>,
    #[clap(short = 't', default_value = "str", requires = "value")]
    value_type: ValType,
    #[arg(long, conflicts_with_all=["key","value"])]
    hexstring: bool,
}

impl TryFrom<RequestPayload> for ciborium::value::Value {
    type Error = ciborium::value::Error;

    fn try_from(req_payload: RequestPayload) -> Result<Self, Self::Error> {
        if req_payload.hexstring {
            let stdin = std::io::stdin();
            let stdin = stdin.lock();
            let mut stdin = BufReader::new(stdin);
            let mut hexstring = Vec::new();
            let _result = stdin.read_to_end(&mut hexstring);

            let bytestring: Vec<_> = hexstring
                .chunks(2)
                .filter_map(|i| u8::from_str_radix(std::str::from_utf8(i).unwrap(), 16).ok())
                .collect();

            ciborium::de::from_reader(&bytestring[..])
                .map_err(|e| Self::Error::Custom(e.to_string()))
        } else if let (Some(key), Some(value)) = (req_payload.key, req_payload.value) {
            match req_payload.value_type {
                ValType::StringType => Ok(ciborium::value::Value::Map(
                    [(
                        ciborium::value::Value::Text(key),
                        ciborium::value::Value::Text(value),
                    )]
                    .to_vec(),
                )),
                ValType::IntType => Ok(ciborium::value::Value::Map(
                    [(
                        ciborium::value::Value::Text(key),
                        ciborium::value::Value::Integer(ciborium::value::Integer::from(
                            value.parse::<i32>().unwrap(),
                        )),
                    )]
                    .to_vec(),
                )),
                ValType::BinType => {
                    let bin_vec = value
                        .chars()
                        .collect::<Vec<char>>()
                        .chunks(2)
                        .map(|c| c.iter().collect::<String>())
                        .map(|c| u8::from_str_radix(&c, 16).unwrap())
                        .collect::<Vec<u8>>();

                    Ok(ciborium::value::Value::Map(
                        [(
                            ciborium::value::Value::Text(key),
                            ciborium::value::Value::Bytes(bin_vec),
                        )]
                        .to_vec(),
                    ))
                }
                ValType::BoolType => Ok(ciborium::value::Value::Map(
                    [(
                        ciborium::value::Value::Text(key),
                        ciborium::value::Value::Bool(value.parse::<bool>().unwrap()),
                    )]
                    .to_vec(),
                )),
            }
        } else {
            Err(Self::Error::Custom(
                "Missing data in the passed structure".to_string(),
            ))
        }
    }
}

#[derive(Args, Debug)]
struct CoapFotaReq {
    #[clap(short, long)]
    addr: String,
}

#[tokio::main]
async fn main() {
    let opts = Opts::parse();
    println!("{:?}", opts);

    let coap = home_mng::Coap::new();

    match opts.subcmd {
        SubCommand::ServiceDiscovery(sd_filter) => {
            if sd_filter.single {
                let result = coap
                    .service_discovery_single(
                        &sd_filter.service_name.unwrap(),
                        sd_filter.service_type.as_deref(),
                    )
                    .await;
                println!("Discovery result: {:?}", result);
            } else {
                let result = coap
                    .service_discovery(
                        sd_filter.service_name.as_deref(),
                        sd_filter.service_type.as_deref(),
                    )
                    .await;

                if let Ok(services) = result {
                    for service in services {
                        println!("{}: {:?}: {:?}", service.0, service.1, service.2);
                    }
                }
            }
        }

        SubCommand::NotProvisioned => {
            let result = coap.not_provisioned_discovery().await;
            println!("{:?}", result);
        }

        SubCommand::Provision(prov) => {
            let data_map = prov
                .req_payload
                .try_into()
                .expect("Missing data to provision. Check command line arguments");

            let _ = coap
                .provision(&get_socket_addr(&prov.addr), &data_map)
                .await;
        }

        SubCommand::ResetProvisioning(prov) => {
            let _ = coap
                .reset_provisioning(&get_socket_addr(&prov.addr), &prov.key)
                .await;
        }

        SubCommand::Get(data) => {
            let payload_map = data.req_payload.try_into().ok();

            let result = coap
                .get(
                    &get_socket_addr(&data.addr),
                    &data.resource,
                    payload_map.as_ref(),
                )
                .await;
            println!("{:?}", result);
        }

        SubCommand::Set(data) => {
            let data_map = data
                .req_payload
                .try_into()
                .expect("Missing data to set. Check command line arguments");

            let sock_addr = get_socket_addr(&data.addr);
            let resource = &data.resource;

            if data.non_confirmable {
                let _ = coap
                    .set_non_confirmable(&sock_addr, resource, &data_map)
                    .await;
            } else {
                let _ = coap.set(&sock_addr, resource, &data_map).await;
            }
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
            let local_addr = format!("[{}]", local_addr);

            let _ = coap
                .fota_req(&get_socket_addr(&data.addr), &local_addr)
                .await;
        }
    }
}

fn get_socket_addr(addr: &str) -> SocketAddr {
    SocketAddr::new(IpAddr::from_str(addr).unwrap(), 5683)
}
