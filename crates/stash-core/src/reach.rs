//! Where an address is: on this computer, on the listener's own network, or out on the internet. An add-on may send
//! Stash only as near as its own link is: one on the internet may name addresses on the internet only, one on the
//! listener's network may name those and the internet, and one on this computer may name anything. So an add-on
//! from the internet cannot make Stash reach into the listener's network (a router's page, a printer, another
//! program on this computer), by an address it names, by a name that leads there, or by a redirect.
//!
//! The core holds to it on every connection it makes for an add-on: its resolver keeps only the addresses the
//! add-on may reach, so a name that leads somewhere nearer is refused however it was named, and every redirect is
//! looked up the same way. What the engine fetches by itself (a playlist of pieces, an encrypted stream) is checked
//! before it is handed over; covers the window shows by themselves are checked by the address they name.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex};

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
    if let Some(v4) = ip.to_ipv4_mapped() {
        return of_v4(v4);
    }
    if ip.is_unspecified() || ip.is_multicast() {
        return None;
    }
    if ip.is_loopback() {
        return Some(Reach::Here);
    }
    let first = ip.segments()[0];
    let unique_local = first & 0xfe00 == 0xfc00;
    let link_local = first & 0xffc0 == 0xfe80;
    if unique_local || link_local {
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
/// it is nearer. A name that only a lookup tells passes here (see [`looked_up`]).
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

/// Whether an add-on of `allowed` may name `url`, its name looked up: for what the engine fetches by itself. (A
/// name's answer can change between this look and the engine's own: the engine's fetches are checked once, here.)
pub fn looked_up(url: &Url, allowed: Reach) -> Result<(), TooNear> {
    as_written(url, allowed)?;
    if literal(url).is_some() || allowed == Reach::Here {
        return Ok(());
    }
    let host = url.host_str().unwrap_or_default();
    let port = url.port_or_known_default().unwrap_or(443);
    // A name that cannot be looked up now goes nowhere: the engine finds that out by itself.
    let Ok(found) = (host, port).to_socket_addrs() else {
        return Ok(());
    };
    let found: Vec<SocketAddr> = found.collect();
    if found.is_empty() {
        return Ok(());
    }
    // All of its addresses: the engine may connect to any of them.
    let kept = keep_reachable(host, found.iter().copied(), allowed)?;
    if kept.len() < found.len() {
        let near = found
            .iter()
            .filter_map(|a| Reach::of_ip(a.ip()))
            .filter(|r| !allowed.allows(*r))
            .max();
        return Err(TooNear {
            host: host.to_string(),
            target: near,
            allowed,
        });
    }
    Ok(())
}

/// The resolver of every connection the core makes for an add-on of `allowed`: names are looked up as usual, and only
/// the addresses the add-on may reach are kept. A name that could not be looked up is said as such.
#[derive(Debug)]
pub struct Guarded {
    allowed: Reach,
    /// Where the addresses it gave out are, when it is asked to note that.
    seen: Option<Arc<Seen>>,
    inner: ureq::unversioned::resolver::DefaultResolver,
}

/// Where the addresses a guard gave out to connect to are: the farthest of them.
#[derive(Debug, Default)]
pub struct Seen(Mutex<Option<Reach>>);

impl Seen {
    /// The farthest place of the addresses given out; none before any was.
    pub fn farthest(&self) -> Option<Reach> {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn note(&self, reach: Reach) {
        let mut seen = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *seen = Some(seen.map_or(reach, |s| s.min(reach)));
    }
}

impl Guarded {
    pub fn new(allowed: Reach) -> Self {
        Self {
            allowed,
            seen: None,
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

    /// An agent for the listener's own link (an add-on's manifest), which may go anywhere, and what it notes of where
    /// it went: where an add-on is, is where its manifest came from. (A name looked up apart from the fetch could
    /// answer one place to the look and another to the fetch.) Of several addresses, the farthest decides.
    pub fn noting(config: ureq::config::Config) -> (ureq::Agent, Arc<Seen>) {
        let seen = Arc::new(Seen::default());
        let guard = Guarded {
            seen: Some(seen.clone()),
            ..Guarded::new(Reach::Here)
        };
        let agent = ureq::Agent::with_parts(
            config,
            ureq::unversioned::transport::DefaultConnector::default(),
            guard,
        );
        (agent, seen)
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
        let kept = keep_reachable(host, found.iter().copied(), self.allowed)
            .map_err(|refused| ureq::Error::Other(Box::new(refused)))?;
        let mut out = self.empty();
        for address in kept {
            if let (Some(seen), Some(reach)) = (&self.seen, Reach::of_ip(address.ip())) {
                seen.note(reach);
            }
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

        // Where an add-on is, is where its manifest came from: noted by the fetch itself.
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
        let (agent, seen) = Guarded::noting(ureq::config::Config::default());
        assert_eq!(seen.farthest(), None);
        agent
            .get(&format!("http://127.0.0.1:{port}/manifest.json"))
            .call()
            .unwrap();
        assert_eq!(seen.farthest(), Some(Here));
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
        // A name that leads to this computer is refused once looked up, as the engine would reach it.
        assert!(looked_up(&url("http://localhost:9/x"), Reach::Home).is_err());
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
