//! Where this machine can be reached, as the Settings panel lists it.
//!
//! Upstream computes this in its Electron main process and hands it to the
//! renderer over `desktopBridge.getAdvertisedEndpoints()`. That process is not
//! in `reference/` — only the server is — so what is copied here is the
//! **contract**, `AdvertisedEndpoint` in `packages/contracts/src/remoteAccess.ts`,
//! which is exact about every field. The rendering is
//! `ConnectionsSettings.tsx`, and it is the reason two details below are not
//! free choices:
//!
//! - **The `id` prefixes matter.** `endpointDefaultPreferenceKey` reads
//!   `desktop-loopback:` and `desktop-lan:` off the front of the id to decide
//!   which stored preference a row is the default for. An id of this module's
//!   invention would still render, and "Set as default" would then remember a
//!   key nothing matches.
//! - **`status` decides whether a row is offered at all.**
//!   `selectPairingEndpoint` skips anything `unavailable`, so an endpoint that
//!   cannot actually be reached must say so rather than be omitted — the row is
//!   how the user finds out it needs setting up.
//!
//! ## What is not here
//!
//! **Tailscale.** Upstream advertises a MagicDNS name and drives `tailscale
//! serve` through the same bridge, and `ConnectionsSettings` has a whole row and
//! two dialogs for it. laplus advertises no such endpoint, so that row does not
//! appear. A tailnet name still reaches this server, and needs nothing written
//! down to do it — the same as a `trycloudflare.com` name, since this server no
//! longer keeps a list of the origins it will hear from.

use serde_json::{json, Value};

use crate::remote_access::RemoteAccess;

/// The address other machines on this network reach this one at.
///
/// Found by asking the operating system which local address it *would* send
/// from, which is a routing-table lookup and not a conversation: `connect` on a
/// UDP socket sends nothing. The destination is TEST-NET-3
/// (`203.0.113.0/24`), reserved by RFC 5737 for exactly this kind of
/// documentation-shaped use, so nothing here can be mistaken for reaching a
/// real host.
///
/// The alternative is enumerating interfaces, which needs a dependency and then
/// needs a rule for choosing between the six adapters a Windows machine with
/// Hyper-V, WSL and a VPN reports. The routing table already holds that answer
/// and is the same one the user's other traffic takes.
///
/// `None` on a machine with no route off itself, which is a laptop with the
/// Wi-Fi off — and is why the LAN row can be absent rather than wrong.
pub fn lan_address() -> Option<std::net::Ipv4Addr> {
    let socket = std::net::UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(("203.0.113.1", 80)).ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(address) if !address.is_loopback() && !address.is_unspecified() => {
            Some(address)
        }
        _ => None,
    }
}

/// Every address this server answers on, in the shape the page renders.
///
/// Two at most: this machine, and this machine on its network. The second is
/// present only when the switch is on — an endpoint listed while the server is
/// bound to loopback would be a URL that refuses every connection made to it.
pub fn advertised(access: &RemoteAccess, port: u16) -> Vec<Value> {
    let mut endpoints = vec![endpoint(
        "desktop-loopback",
        "This machine",
        "loopback",
        "127.0.0.1",
        port,
        true,
    )];

    if access.exposure().is_network_accessible() {
        if let Some(address) = lan_address() {
            endpoints.push(endpoint(
                "desktop-lan",
                "Local network",
                "lan",
                &address.to_string(),
                port,
                false,
            ));
        }
    }

    endpoints
}

/// The host a pairing URL should name, or `None` to leave it to the page.
///
/// Upstream's `DesktopServerExposureState.advertisedHost`. The LAN address when
/// there is one, because a code carried to a phone is useless pointing at
/// `127.0.0.1`.
pub fn advertised_host(access: &RemoteAccess) -> Option<String> {
    access
        .exposure()
        .is_network_accessible()
        .then(lan_address)
        .flatten()
        .map(|address| address.to_string())
}

fn endpoint(
    id_prefix: &str,
    label: &str,
    reachability: &str,
    host: &str,
    port: u16,
    is_default: bool,
) -> Value {
    json!({
        "id": format!("{id_prefix}:{host}:{port}"),
        "label": label,
        "provider": {
            "id": "desktop-core",
            "label": "laplus",
            // `core` and not `private-network`: both of these are this server
            // answering on its own listener. The other kinds describe something
            // standing in front of it, and nothing does.
            "kind": "core",
            "isAddon": false,
        },
        "httpBaseUrl": format!("http://{host}:{port}"),
        "wsBaseUrl": format!("ws://{host}:{port}"),
        "reachability": reachability,
        "compatibility": {
            // Plain HTTP, so a page served over HTTPS cannot call it — the
            // browser blocks the mixed content before this server is reached.
            // Upstream's hosted app is what that field is about and ticket 72
            // removed it; the value is still the truthful one.
            "hostedHttpsApp": "mixed-content-blocked",
            "desktopApp": "compatible",
        },
        "source": "desktop-core",
        "status": "available",
        "isDefault": is_default,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_access::Exposure;

    #[test]
    fn a_loopback_server_advertises_only_itself() {
        let endpoints = advertised(&RemoteAccess::none(), 4773);
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0]["reachability"], json!("loopback"));
        assert_eq!(endpoints[0]["httpBaseUrl"], json!("http://127.0.0.1:4773"));
    }

    /// The prefixes `endpointDefaultPreferenceKey` reads. Pinned because the
    /// cost of getting one wrong is not a broken render — it is "Set as
    /// default" storing a key that never matches a row again.
    #[test]
    fn the_ids_carry_the_prefixes_the_page_keys_its_preference_on() {
        let loopback = advertised(&RemoteAccess::none(), 4773);
        assert!(
            loopback[0]["id"]
                .as_str()
                .expect("an id")
                .starts_with("desktop-loopback:"),
            "{loopback:?}"
        );

        let networked = advertised(
            &RemoteAccess::none().with_exposure(Exposure::NetworkAccessible),
            4773,
        );
        // The LAN row is absent on a machine with no route off itself, which is
        // a real state and not a failure — so this asserts about the row only
        // when there is one, rather than requiring the suite to have a network.
        if let Some(lan) = networked.get(1) {
            assert!(
                lan["id"].as_str().expect("an id").starts_with("desktop-lan:"),
                "{lan:?}"
            );
            assert_eq!(lan["reachability"], json!("lan"));
        }
    }

    /// A URL that would refuse every connection made to it is worse than no
    /// URL: the user copies it, carries it to a phone, and learns nothing about
    /// why it failed.
    #[test]
    fn a_lan_endpoint_is_not_offered_while_the_server_is_bound_to_loopback() {
        let endpoints = advertised(&RemoteAccess::none(), 4773);
        assert!(
            !endpoints
                .iter()
                .any(|endpoint| endpoint["reachability"] == json!("lan")),
            "{endpoints:?}"
        );
        assert_eq!(advertised_host(&RemoteAccess::none()), None);
    }
}
