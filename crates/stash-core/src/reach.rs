//! Where an address is: on this computer, on the listener's own network, or out on the internet. An add-on may send
//! Stash only as near as its own link is: one on the internet may name addresses on the internet only, one on the
//! listener's network may name those and the internet, and one on this computer may name anything. So an add-on
//! from the internet cannot make Stash reach into the listener's network (a router's page, a printer, another
//! program on this computer), by an address it names, by a name that leads there, or by a redirect.
//!
//! The core holds to it on every connection it makes for an add-on: its resolver keeps only the addresses the
//! add-on may reach, so a name that leads somewhere nearer is refused however it was named, and every redirect is
//! looked up the same way. The engine never fetches an add-on's address by itself: it is given the relay's, and the
//! relay's connections are the core's. Covers the window shows are checked by the address they name.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use serde::{Deserialize, Serialize};
use url::Url;

/// Where an address is, farthest first: an add-on of one reach may name addresses of that reach and those farther.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reach {
    /// Out on the internet.
    Internet,
    /// On the listener's own network: a home or office network, a link-local address, the network a provider
    /// shares among its customers.
    Home,
    /// On this computer.
    Here,
}

impl Reach {
    /// Whether an add-on of this reach may send Stash to an address of `target`'s.
    pub fn allows(self, target: Reach) -> bool {
        target <= self
    }

    /// For the listener: "on the internet", "on your network", "on this computer".
    pub fn words(self) -> &'static str {
        match self {
            Reach::Internet => "on the internet",
            Reach::Home => "on your network",
            Reach::Here => "on this computer",
        }
    }

    /// Where an address is; none for an address that is no place at all (unspecified, multicast, broadcast,
    /// reserved), which no add-on may name.
    pub fn of_ip(ip: IpAddr) -> Option<Reach> {
        match ip {
            IpAddr::V4(ip) => of_v4(ip),
            IpAddr::V6(ip) => of_v6(ip),
        }
    }

    /// Where a name is by its form alone: "localhost" is this computer; a name of one word, or one under a suffix
    /// kept for home networks (.local, .lan, .home, .internal, .home.arpa), is on the listener's network. None for
    /// any other name: only looking it up tells.
    pub fn of_name(name: &str) -> Option<Reach> {
        let name = name.trim_end_matches('.').to_ascii_lowercase();
        if name == "localhost" || name.ends_with(".localhost") {
            return Some(Reach::Here);
        }
        let home = [".local", ".lan", ".home", ".internal", ".home.arpa"];
        if !name.contains('.') || home.iter().any(|suffix| name.ends_with(suffix)) {
            return Some(Reach::Home);
        }
        None
    }

    /// Where a link kept from before reaches were noted is: by its address or its name's form alone, never by a
    /// lookup (a name's answer can change between starts), so any other name is on the internet.
    pub fn of_link_as_written(url: &Url) -> Reach {
        match literal(url) {
            Some(reach) => reach.unwrap_or(Reach::Internet),
            None => Reach::of_name(url.host_str().unwrap_or_default()).unwrap_or(Reach::Internet),
        }
    }
}

/// The reach of a URL whose host is an address (or "localhost"), as written: `Some(None)` for an address that is no
/// place; `None` when the host is a name that only a lookup tells.
fn literal(url: &Url) -> Option<Option<Reach>> {
    match url.host()? {
        url::Host::Ipv4(ip) => Some(Reach::of_ip(IpAddr::V4(ip))),
        url::Host::Ipv6(ip) => Some(Reach::of_ip(IpAddr::V6(ip))),
        url::Host::Domain(name) => Reach::of_name(name).filter(|r| *r == Reach::Here).map(Some),
    }
}

fn of_v4(ip: Ipv4Addr) -> Option<Reach> {
    let [a, b, ..] = ip.octets();
    if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() || a == 0 || a >= 240 {
        return None;
    }
    if ip.is_loopback() {
        return Some(Reach::Here);
    }
    let shared = a == 100 && (64..128).contains(&b);
    if ip.is_private() || ip.is_link_local() || shared {
        return Some(Reach::Home);
    }
    Some(Reach::Internet)
}

fn of_v6(ip: Ipv6Addr) -> Option<Reach> {
    if ip.is_unspecified() || ip.is_multicast() {
        return None;
    }
    if ip.is_loopback() {
        return Some(Reach::Here);
    }
    // An IPv4 address carried in an IPv6 one is where that IPv4 address is: written as IPv6 (::ffff:a.b.c.d, and the
    // old ::a.b.c.d), translated (NAT64, 64:ff9b::/96 and the local 64:ff9b:1::/48), or tunnelled (6to4, 2002::/16).
    let s = ip.segments();
    let last = Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
    let mapped = s[..5] == [0; 5] && (s[5] == 0xffff || s[5] == 0);
    let nat64 = (s[0], s[1]) == (0x64, 0xff9b) && (s[2..6] == [0; 4] || s[2] == 1);
    if mapped || nat64 {
        return of_v4(last);
    }
    if s[0] == 0x2002 {
        return of_v4(Ipv4Addr::new(
            (s[1] >> 8) as u8,
            s[1] as u8,
            (s[2] >> 8) as u8,
            s[2] as u8,
        ));
    }
    let unique_local = s[0] & 0xfe00 == 0xfc00;
    let link_local = s[0] & 0xffc0 == 0xfe80;
    // Site-local (fec0::/10): given up long ago, still where Windows looks for a name server on the network.
    let site_local = s[0] & 0xffc0 == 0xfec0;
    if unique_local || link_local || site_local {
        return Some(Reach::Home);
    }
    Some(Reach::Internet)
}

/// Why a connection was not made: the address an add-on named, or its name led to, is nearer than the add-on may
/// reach.
#[derive(Debug)]
pub struct TooNear {
    pub host: String,
    pub target: Option<Reach>,
    pub allowed: Reach,
}

impl std::fmt::Display for TooNear {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.target {
            Some(target) => write!(
                f,
                "{} is {}, and an add-on {} may not send Stash there",
                self.host,
                target.words(),
                self.allowed.words()
            ),
            None => write!(f, "{} is not an address Stash goes to", self.host),
        }
    }
}

impl std::error::Error for TooNear {}

/// Keeps of `found` the addresses an add-on of `allowed` may reach, or says why there are none.
pub fn keep_reachable(
    host: &str,
    found: impl IntoIterator<Item = SocketAddr>,
    allowed: Reach,
) -> Result<Vec<SocketAddr>, TooNear> {
    let mut nearest = None;
    let mut kept = Vec::new();
    for address in found {
        match Reach::of_ip(address.ip()) {
            Some(reach) if allowed.allows(reach) => kept.push(address),
            other => nearest = Some(nearest.map_or(other, |n: Option<Reach>| n.max(other))),
        }
    }
    if kept.is_empty() {
        return Err(TooNear {
            host: host.to_string(),
            target: nearest.flatten(),
            allowed,
        });
    }
    Ok(kept)
}

/// Whether an add-on of `allowed` may name `url`, by what is written in it: its address, or a name whose form says
/// it is nearer. A name that only a lookup tells passes here: the connections made for it are guarded ([`Guarded`]).
pub fn as_written(url: &Url, allowed: Reach) -> Result<(), TooNear> {
    let target = match literal(url) {
        Some(target) => target,
        None => match Reach::of_name(url.host_str().unwrap_or_default()) {
            Some(target) => Some(target),
            None => return Ok(()),
        },
    };
    match target {
        Some(target) if allowed.allows(target) => Ok(()),
        _ => Err(TooNear {
            host: url.host_str().unwrap_or_default().to_string(),
            target,
            allowed,
        }),
    }
}

/// The resolver of every connection the core makes for an add-on of `allowed`: names are looked up as usual, and only
/// the addresses the add-on may reach are kept. A name that could not be looked up is said as such.
#[derive(Debug)]
pub struct Guarded {
    allowed: Reach,
    /// Only the addresses of this place, when it is asked for (see [`Guarded::only`]).
    only: Option<Reach>,
    inner: ureq::unversioned::resolver::DefaultResolver,
}

/// Why a guard that keeps one place's addresses connected nowhere: the name has none there.
#[derive(Debug)]
pub struct NoneThere;

impl std::fmt::Display for NoneThere {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the name has no address there")
    }
}

impl std::error::Error for NoneThere {}

/// Whether `e` is a guard's that kept one place's addresses and found none.
pub fn none_there(e: &ureq::Error) -> bool {
    matches!(e, ureq::Error::Other(inner) if inner.is::<NoneThere>())
}

impl Guarded {
    pub fn new(allowed: Reach) -> Self {
        Self {
            allowed,
            only: None,
            inner: ureq::unversioned::resolver::DefaultResolver::default(),
        }
    }

    /// An agent built from `config` whose every connection is guarded so.
    pub fn agent(allowed: Reach, config: ureq::config::Config) -> ureq::Agent {
        ureq::Agent::with_parts(
            config,
            ureq::unversioned::transport::DefaultConnector::default(),
            Guarded::new(allowed),
        )
    }

    /// An agent that connects only to the addresses of `place`: for the listener's own link (an add-on's manifest),
    /// asked of each place in turn, farthest first. Where it answers is where the add-on is. (A name looked up apart
    /// from the fetch could answer one place to the look and another to the fetch; and an add-on on the listener's
    /// network whose name has an address on the internet too, where nothing answers, is where it answers. Over https
    /// only the add-on's own server can answer for its name: another place cannot pass for it.)
    pub fn only(place: Reach, config: ureq::config::Config) -> ureq::Agent {
        let guard = Guarded {
            only: Some(place),
            ..Guarded::new(Reach::Here)
        };
        ureq::Agent::with_parts(
            config,
            ureq::unversioned::transport::DefaultConnector::default(),
            guard,
        )
    }
}

impl ureq::unversioned::resolver::Resolver for Guarded {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let found = self
            .inner
            .resolve(uri, config, timeout)
            .map_err(|e| match e {
                // The system says a name could not be looked up with an error of no kind of its own.
                ureq::Error::Io(_) => ureq::Error::HostNotFound,
                other => other,
            })?;
        let host = uri.host().unwrap_or_default();
        let mut kept = keep_reachable(host, found.iter().copied(), self.allowed)
            .map_err(|refused| ureq::Error::Other(Box::new(refused)))?;
        if let Some(place) = self.only {
            kept.retain(|a| Reach::of_ip(a.ip()) == Some(place));
            if kept.is_empty() {
                return Err(ureq::Error::Other(Box::new(NoneThere)));
            }
        }
        let mut out = self.empty();
        for address in kept {
            out.push(address);
        }
        Ok(out)
    }
}

/// The add-on's own words for a connection the guard refused, when that is why `e` happened.
pub fn refusal(e: &ureq::Error) -> Option<String> {
    match e {
        ureq::Error::Other(inner) => inner.downcast_ref::<TooNear>().map(ToString::to_string),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> Option<Reach> {
        Reach::of_ip(text.parse().unwrap())
    }

    #[test]
    fn every_address_is_somewhere_or_nowhere() {
        for (address, reach) in [
            ("127.0.0.1", Some(Reach::Here)),
            ("127.9.9.9", Some(Reach::Here)),
            ("::1", Some(Reach::Here)),
            ("::ffff:127.0.0.1", Some(Reach::Here)),
            ("10.1.2.3", Some(Reach::Home)),
            ("172.16.0.1", Some(Reach::Home)),
            ("172.31.255.255", Some(Reach::Home)),
            ("192.168.1.1", Some(Reach::Home)),
            ("169.254.169.254", Some(Reach::Home)),
            ("100.64.0.1", Some(Reach::Home)),
            ("100.127.255.254", Some(Reach::Home)),
            ("fd12:3456::1", Some(Reach::Home)),
            ("fe80::1", Some(Reach::Home)),
            ("::ffff:192.168.0.5", Some(Reach::Home)),
            ("::127.0.0.1", Some(Reach::Here)),
            ("::c0a8:101", Some(Reach::Home)),
            ("64:ff9b::7f00:1", Some(Reach::Here)),
            ("64:ff9b::c0a8:101", Some(Reach::Home)),
            ("64:ff9b:1::a00:1", Some(Reach::Home)),
            ("64:ff9b::808:808", Some(Reach::Internet)),
            ("2002:c0a8:101::1", Some(Reach::Home)),
            ("2002:7f00:1::1", Some(Reach::Here)),
            ("2002:808:808::1", Some(Reach::Internet)),
            ("fec0:0:0:ffff::1", Some(Reach::Home)),
            ("172.32.0.1", Some(Reach::Internet)),
            ("100.128.0.1", Some(Reach::Internet)),
            ("8.8.8.8", Some(Reach::Internet)),
            ("2606:4700::1111", Some(Reach::Internet)),
            ("0.0.0.0", None),
            ("0.1.2.3", None),
            ("::", None),
            ("224.0.0.1", None),
            ("ff02::1", None),
            ("255.255.255.255", None),
            ("240.0.0.1", None),
        ] {
            assert_eq!(ip(address), reach, "{address}");
        }
    }

    #[test]
    fn a_name_s_form_tells_only_of_this_computer_and_home_networks() {
        assert_eq!(Reach::of_name("localhost"), Some(Reach::Here));
        assert_eq!(Reach::of_name("LocalHost."), Some(Reach::Here));
        assert_eq!(Reach::of_name("app.localhost"), Some(Reach::Here));
        for home in [
            "nas",
            "nas.local",
            "music.lan",
            "box.home.arpa",
            "srv.internal",
            "pi.home",
        ] {
            assert_eq!(Reach::of_name(home), Some(Reach::Home), "{home}");
        }
        assert_eq!(Reach::of_name("addon.example.org"), None);
    }

    #[test]
    fn an_add_on_reaches_as_far_as_its_own_link_and_no_nearer() {
        use Reach::*;
        assert!(Internet.allows(Internet) && !Internet.allows(Home) && !Internet.allows(Here));
        assert!(Home.allows(Internet) && Home.allows(Home) && !Home.allows(Here));
        assert!(Here.allows(Internet) && Here.allows(Home) && Here.allows(Here));
        let url = |text: &str| Url::parse(text).unwrap();
        assert_eq!(
            Reach::of_link_as_written(&url("https://nas.local/manifest.json")),
            Home
        );
        assert_eq!(
            Reach::of_link_as_written(&url("http://localhost:1/manifest.json")),
            Here
        );

        // Where an add-on is, is where its manifest answers: each place asked in turn.
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut asked = [0u8; 1024];
            let _ = socket.read(&mut asked);
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
        });
        let link = format!("http://127.0.0.1:{port}/manifest.json");
        for place in [Internet, Home] {
            let e = Guarded::only(place, ureq::config::Config::default())
                .get(&link)
                .call()
                .unwrap_err();
            assert!(none_there(&e), "{place:?}: {e}");
        }
        Guarded::only(Here, ureq::config::Config::default())
            .get(&link)
            .call()
            .unwrap();
        assert_eq!(
            Reach::of_link_as_written(&url("https://addon.example.org/manifest.json")),
            Internet
        );
        assert_eq!(
            Reach::of_link_as_written(&url("https://10.0.0.2/manifest.json")),
            Home
        );
    }

    #[test]
    fn what_an_add_on_names_is_held_to_its_reach() {
        let url = |text: &str| Url::parse(text).unwrap();
        let internet = Reach::Internet;
        assert!(as_written(&url("https://cdn.example.org/a.flac"), internet).is_ok());
        for near in [
            "https://192.168.1.1/admin",
            "http://127.0.0.1:47800/api/player",
            "http://localhost:9/",
            "https://[::1]/x",
            "https://[fe80::1]/x",
            "https://router.lan/",
            "https://0.0.0.0/x",
        ] {
            let refused = as_written(&url(near), internet).unwrap_err();
            assert!(
                refused.to_string().contains("may not send Stash there")
                    || refused.target.is_none(),
                "{near}: {refused}"
            );
        }
        assert!(as_written(&url("https://192.168.1.1/x"), Reach::Home).is_ok());
        assert!(as_written(&url("http://127.0.0.1:1/x"), Reach::Home).is_err());
        assert!(as_written(&url("http://127.0.0.1:1/x"), Reach::Here).is_ok());
        assert_eq!(
            as_written(&url("https://192.168.1.1/x"), internet)
                .unwrap_err()
                .to_string(),
            "192.168.1.1 is on your network, and an add-on on the internet may not send Stash there"
        );
    }

    #[test]
    fn the_guard_keeps_only_the_addresses_an_add_on_may_reach() {
        let a = |text: &str| -> SocketAddr { text.parse().unwrap() };
        let found = [a("10.0.0.7:443"), a("93.184.216.34:443")];
        assert_eq!(
            keep_reachable("mixed.example", found, Reach::Internet).unwrap(),
            [a("93.184.216.34:443")]
        );
        let refused =
            keep_reachable("rebound.example", [a("127.0.0.1:443")], Reach::Internet).unwrap_err();
        assert_eq!(
            refused.to_string(),
            "rebound.example is on this computer, and an add-on on the internet may not send Stash there"
        );
        assert_eq!(
            keep_reachable("local", [a("127.0.0.1:1")], Reach::Here)
                .unwrap()
                .len(),
            1
        );

        // Through the resolver, as every connection is made: a name that leads here is refused, said in words.
        use ureq::unversioned::resolver::Resolver;
        let guard = Guarded::new(Reach::Internet);
        let config = ureq::config::Config::default();
        let uri: ureq::http::Uri = "http://127.0.0.1:9/x".parse().unwrap();
        let timeout = ureq::unversioned::transport::NextTimeout {
            after: ureq::unversioned::transport::time::Duration::NotHappening,
            reason: ureq::Timeout::Resolve,
            per_read: None,
            per_write: None,
        };
        let e = guard.resolve(&uri, &config, timeout).unwrap_err();
        assert_eq!(
            refusal(&e).as_deref(),
            Some(
                "127.0.0.1 is on this computer, and an add-on on the internet may not send Stash there"
            )
        );
    }
}
