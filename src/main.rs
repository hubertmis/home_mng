use clap::Parser;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

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

impl ToString for ValType {
    fn to_string(&self) -> String {
        match self {
            ValType::StringType => "str",
            ValType::IntType => "int",
            ValType::BinType => "bin",
            ValType::BoolType => "bool",
        }.to_string()
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
    #[clap(long)]
    keys: Option<String>,
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

fn encode_req_payload(key: String, value: String, keys:Option<String>, values: Option<String>, value_type: ValType) -> ciborium::value::Value {
    let result: ciborium::value::Value;

    match value_type {
        ValType::StringType => {
            match (keys, values) {
                (Some(_), Some(_)) => result = ciborium::value::Value::Null,
                (Some(keys), None) => {
                    let mut key_results_vec: Vec<ciborium::value::Value> = Vec::new();
                    for k in keys.split(',') {
                        key_results_vec.push(k.into());
                    }
                    result = ciborium::value::Value::Array(key_results_vec);
                }
                (_, _) => result = ciborium::value::Value::Map([(
                                ciborium::value::Value::Text(key),
                                ciborium::value::Value::Text(value))].to_vec()),
            }
        }
        ValType::IntType => {
            if let (Some(keys), Some(values)) = (keys, values) {
                let key_iter = keys.split(',');
                let value_iter = values.split(',');
                let mut vec_pairs = Vec::new();
                for (k, v) in key_iter.zip(value_iter) {
                    vec_pairs.push((
                            ciborium::value::Value::Text(k.to_string()),
                            ciborium::value::Value::Integer(
                                ciborium::value::Integer::from(
                                    v.parse::<i32>().unwrap()
                                )
                            )));
                }
                result = ciborium::value::Value::Map(vec_pairs);
            } else {
                result = ciborium::value::Value::Map([(
                        ciborium::value::Value::Text(key),
                        ciborium::value::Value::Integer(
                            ciborium::value::Integer::from(
                                value.parse::<i32>().unwrap()
                            )
                        ))].to_vec());
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

                result = ciborium::value::Value::Map([(
                        ciborium::value::Value::Text(key),
                        ciborium::value::Value::Bytes(bin_vec)
                        )].to_vec());
        }
        ValType::BoolType => {
                result = ciborium::value::Value::Map([(
                        ciborium::value::Value::Text(key),
                        ciborium::value::Value::Bool(
                            value.parse::<bool>().unwrap()
                        ))].to_vec());
        }
    }

    result
}

#[tokio::main]
async fn main() {
    let opts = Opts::parse();
    println!("{:?}", opts);

    let coap = home_mng::Coap::new();

    match opts.subcmd {
        SubCommand::ServiceDiscovery(sd_filter) => {
            let result = coap.service_discovery(sd_filter.service_name.as_deref(), sd_filter.service_type.as_deref()).await;

            if let Ok(services) = result {
                for service in services {
                    println!("{}: {:?}: {:?}", service.0, service.1, service.2);
                }
            }
        }

        SubCommand::NotProvisioned => {
            let _ = coap.not_provisioned_discovery().await;
        }

        SubCommand::Provision(prov) => {
            let value = home_mng::Value::from_type_and_str(&prov.value_type.to_string(), &prov.value).unwrap();
            let _ = coap.provision(&get_socket_addr(&prov.addr), &prov.key, &value).await;
        }

        SubCommand::ResetProvisioning(prov) => {
            let _ = coap.reset_provisioning(&get_socket_addr(&prov.addr), &prov.key).await;
        }

        SubCommand::Get(data) => {
            let payload_map;

            if let (Some(key), Some(val)) = (data.key, data.value) {
                payload_map = Some(encode_req_payload(key, val, data.keys, None, data.value_type));
            } else {
                payload_map = None;
            }

            let result = coap.get(&get_socket_addr(&data.addr), &data.resource, payload_map.as_ref()).await;
            println!("{:?}", result);
        }

        SubCommand::Set(data) => {
            let data_map = encode_req_payload(data.key, data.value, data.keys, data.values, data.value_type);

            let _ = coap.set(&get_socket_addr(&data.addr), &data.resource, &data_map).await;
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

            let _ = coap.fota_req(&get_socket_addr(&data.addr), &local_addr).await;
        }
    }
}

fn get_socket_addr(addr: &str) -> SocketAddr {
    SocketAddr::new(IpAddr::from_str(addr).unwrap(), 5683)
}
