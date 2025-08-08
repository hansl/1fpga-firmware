use boa_engine::{js_string, Context, JsResult, JsString, Module};
use boa_macros::{boa_module, TryIntoJs};
use nix::ifaddrs::InterfaceAddress;
use nix::sys::socket::{AddressFamily, SockaddrLike, SockaddrStorage};
use std::net::IpAddr;

#[derive(Clone, Debug, TryIntoJs)]
pub struct NetworkInterface {
    ty_: String,
    flags: Vec<String>,
    name: String,
    address: Option<String>,
    netmask: Option<String>,
    family: Option<String>,
}

fn opt_addr_to_ip_addr(addr: Option<SockaddrStorage>) -> Option<IpAddr> {
    addr.and_then(|addr| match addr.family() {
        Some(AddressFamily::Inet) => Some(IpAddr::V4(addr.as_sockaddr_in().unwrap().ip())),
        Some(AddressFamily::Inet6) => Some(IpAddr::V6(addr.as_sockaddr_in6().unwrap().ip())),
        _ => None,
    })
}

impl From<InterfaceAddress> for NetworkInterface {
    fn from(value: InterfaceAddress) -> Self {
        let flags = value.flags;
        let ty_ = if flags.contains(nix::net::if_::InterfaceFlags::IFF_LOOPBACK) {
            "Loopback"
        } else if flags.contains(nix::net::if_::InterfaceFlags::IFF_UP) {
            "Up"
        } else {
            "Down"
        };

        let name = value.interface_name.to_string();
        let address = opt_addr_to_ip_addr(value.address).map(|ip| ip.to_string());
        let netmask = opt_addr_to_ip_addr(value.netmask).map(|ip| ip.to_string());
        let family = if value
            .address
            .map(|a| a.family() == Some(AddressFamily::Inet))
            .unwrap_or(false)
        {
            Some("IPv4".to_string())
        } else if value
            .address
            .map(|a| a.family() == Some(AddressFamily::Inet6))
            .unwrap_or(false)
        {
            Some("IPv6".to_string())
        } else {
            None
        };

        NetworkInterface {
            ty_: ty_.to_string(),
            flags: flags
                .iter()
                .map(|f| format!("{:?}", f))
                .collect::<Vec<String>>(),
            name,
            address,
            netmask,
            family,
        }
    }
}

#[boa_module]
#[boa(rename_all = "camelCase")]
mod js {
    use boa_engine::object::builtins::JsPromise;
    use boa_engine::value::TryIntoJs;
    use boa_engine::{Context, JsError, JsResult, JsString, JsValue};
    use reqwest::header::CONTENT_DISPOSITION;
    use std::path::PathBuf;
    use std::str::FromStr;
    use std::time::Duration;

    fn interfaces(ctx: &mut Context) -> JsResult<JsPromise> {
        let addrs = nix::ifaddrs::getifaddrs().map_err(JsError::from_rust)?;
        let result = addrs
            .map(super::NetworkInterface::from)
            .filter(|i| i.family.is_some() || i.address.is_some())
            .collect::<Vec<_>>();

        result
            .into_iter()
            .map(|i| i.try_into_js(ctx))
            .collect::<JsResult<Vec<_>>>()
            .and_then(|interfaces| Ok(JsPromise::resolve(interfaces.try_into_js(ctx)?, ctx)))
    }

    fn fetch_json(url: String, ctx: &mut Context) -> JsResult<JsPromise> {
        let result = reqwest::blocking::get(&url)
            .map_err(|e| JsError::from_opaque(JsString::from(e.to_string()).into()))?
            .text()
            .map_err(|e| JsError::from_opaque(JsString::from(e.to_string()).into()))
            .and_then(|text| {
                JsValue::from_json(
                    &serde_json::Value::from_str(&text)
                        .map_err(|e| JsError::from_opaque(JsString::from(e.to_string()).into()))?,
                    ctx,
                )
            });
        Ok(match result {
            Ok(v) => JsPromise::resolve(v, ctx),
            Err(e) => JsPromise::reject(e, ctx),
        })
    }

    fn download(url: String, destination: Option<String>) -> JsResult<JsString> {
        let mut response = reqwest::blocking::get(&url)
            .map_err(|e| JsError::from_opaque(JsString::from(e.to_string()).into()))?;

        let file_name = response
            .headers()
            .get(CONTENT_DISPOSITION)
            .and_then(|header| header.to_str().ok())
            .and_then(|header| {
                let parts: Vec<&str> = header.split(';').collect();
                parts.iter().find_map(|part| {
                    if part.trim().starts_with("filename=") {
                        Some(
                            part.trim_start_matches("filename=")
                                .trim_matches('"')
                                .to_string(),
                        )
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_else(|| url.split('/').next_back().unwrap().to_string());

        let path = if let Some(dir) = destination {
            PathBuf::from(dir).join(file_name)
        } else {
            let temp_dir = tempdir::TempDir::new("1fpga")
                .map_err(|e| JsError::from_opaque(JsString::from(e.to_string()).into()))?;
            temp_dir.path().join(file_name)
        };

        std::fs::create_dir_all(path.parent().unwrap())
            .map_err(|e| JsError::from_opaque(JsString::from(e.to_string()).into()))?;
        let mut file = std::fs::File::create(path.clone())
            .map_err(|e| JsError::from_opaque(JsString::from(e.to_string()).into()))?;

        std::io::copy(&mut response, &mut file)
            .map_err(|e| JsError::from_opaque(JsString::from(e.to_string()).into()))?;

        Ok(JsString::from(path.display().to_string()))
    }

    fn is_online(ctx: &mut Context) -> JsPromise {
        let is_online = ping::ping(
            [1, 1, 1, 1].into(),
            Some(Duration::from_secs(1)),
            None,
            None,
            None,
            None,
        )
        .is_ok();
        JsPromise::resolve(is_online, ctx)
    }
}

pub fn create_module(context: &mut Context) -> JsResult<(JsString, Module)> {
    Ok((js_string!("net"), js::boa_module(None, context)))
}
