//! Binary tables and algorithm ported from golang.org/x/net/publicsuffix v0.58.0.
//! Pinned PSL revision d6c92f1bbb7433e5db7b8405c25d4035fb8ff376 (2026-02-06).
//! Go Authors BSD license is bundled with the unmodified table bytes.
const TEXT: &[u8] = include_bytes!("../resources/psl/text");
const NODES: &[u8] = include_bytes!("../resources/psl/nodes");
const CHILDREN: &[u8] = include_bytes!("../resources/psl/children");
fn node(i: usize) -> u64 {
    NODES[i * 5..i * 5 + 5]
        .iter()
        .fold(0, |n, b| (n << 8) | *b as u64)
}
fn children(i: usize) -> u32 {
    u32::from_be_bytes(CHILDREN[i * 4..i * 4 + 4].try_into().unwrap())
}
fn label(i: usize) -> &'static str {
    let n = node(i);
    let len = (n & 63) as usize;
    let offset = ((n >> 6) & 65535) as usize;
    std::str::from_utf8(&TEXT[offset..offset + len]).unwrap()
}
fn find(s: &str, mut lo: usize, mut hi: usize) -> Option<usize> {
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match label(mid).cmp(s) {
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Equal => return Some(mid),
            std::cmp::Ordering::Greater => hi = mid,
        }
    }
    None
}
pub fn public_suffix(domain: &str) -> (&str, bool) {
    if domain.parse::<std::net::IpAddr>().is_ok() {
        return (domain, false);
    }
    let (mut lo, mut hi) = (0, 1450);
    let mut s = domain;
    let mut suffix = domain.len();
    let mut icann = false;
    let mut icann_node = false;
    let mut wildcard = false;
    loop {
        let dot = s.rfind('.');
        let after = dot.map_or(0, |i| i + 1);
        if wildcard {
            icann = icann_node;
            suffix = after;
        }
        if lo == hi {
            break;
        }
        let Some(f) = find(&s[after..], lo, hi) else {
            break;
        };
        let mut u = (node(f) >> 22) as u32;
        icann_node = u & 1 != 0;
        u >>= 1;
        u = children((u & 1023) as usize);
        lo = (u & 16383) as usize;
        u >>= 14;
        hi = (u & 16383) as usize;
        u >>= 14;
        match u & 3 {
            0 => suffix = after,
            1 => {
                suffix = s.len() + 1;
                break;
            }
            _ => {}
        }
        u >>= 2;
        wildcard = u & 1 != 0;
        if !wildcard {
            icann = icann_node;
        }
        let Some(dot) = dot else {
            break;
        };
        s = &s[..dot];
    }
    if suffix == domain.len() {
        (&domain[domain.rfind('.').map_or(0, |i| i + 1)..], icann)
    } else {
        (&domain[suffix..], icann)
    }
}
pub fn root_domain(host: &str) -> String {
    let h = crate::assets::domain_key(host);
    if h.is_empty()
        || h.parse::<std::net::IpAddr>().is_ok()
        || h.starts_with('.')
        || h.ends_with('.')
        || h.contains("..")
    {
        return h;
    }
    let (suffix, _) = public_suffix(&h);
    if h.len() <= suffix.len() {
        return h;
    }
    let i = h.len() - suffix.len() - 1;
    if h.as_bytes()[i] != b'.' {
        return h;
    }
    let offset = h[..i].rfind('.').map_or(0, |j| j + 1);
    h[offset..].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn go_pinned_psl_exact_boundaries() {
        for (h, want) in [
            ("www.example.co.uk", "example.co.uk"),
            ("a.b.github.io", "b.github.io"),
            ("www.city.kawasaki.jp", "city.kawasaki.jp"),
            ("a.b.kawasaki.jp", "a.b.kawasaki.jp"),
            ("foo.bar.ck", "foo.bar.ck"),
            ("www.ck", "www.ck"),
            ("127.0.0.1", "127.0.0.1"),
            ("localhost", "localhost"),
            ("a.unknown", "a.unknown"),
            ("COM", "com"),
            ("foo..com", "foo..com"),
        ] {
            assert_eq!(root_domain(h), want, "{h}");
        }
        assert_eq!(public_suffix("co.uk"), ("co.uk", true));
        assert_eq!(public_suffix("github.io"), ("github.io", false));
    }
    #[test]
    fn all_go_psl_differential_vectors_match() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../resources/psl/go-cases.json")).unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 20666);
        for c in cases {
            let host = c["host"].as_str().unwrap();
            let (suffix, icann) = public_suffix(host);
            assert_eq!(suffix, c["suffix"].as_str().unwrap(), "suffix {host}");
            assert_eq!(icann, c["icann"].as_bool().unwrap(), "ICANN {host}");
            assert_eq!(
                root_domain(host),
                c["root"].as_str().unwrap(),
                "root {host}"
            );
        }
    }
}
