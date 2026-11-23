//! What may leave the device.
//!
//! Every export - the Linux connection log, the Linux diagnostics bundle, the
//! Android zip and the Android copy/share text - goes through this one
//! implementation, so the phone, the laptop and any future port redact the
//! same way.
//!
//! The rules are value-first: a field nobody has thought about is still safe,
//! because every value is classified by what it looks like. Only the peer
//! labels are key-scoped, because a peer identifier is not self-evident from
//! its shape.

use std::net::{Ipv4Addr, Ipv6Addr};

/// A key, a token or any other value that must never leave the device.
pub const REDACTED: &str = "[redacted]";
/// A publicly routable address.
pub const PUBLIC_IP: &str = "<public-ip>";
/// A wireless network name.
pub const SSID: &str = "<ssid>";
/// A device name.
pub const DEVICE: &str = "<device>";
/// A hardware address.
pub const MAC: &str = "<mac>";
/// The name of the machine.
pub const HOST: &str = "<host>";
/// The user's home directory.
pub const HOME: &str = "~";
/// A pairing code.
pub const CODE: &str = "<code>";

/// The longest hex/colon run an address scan will consider. A longer run is
/// not an address, and re-parsing every prefix of a huge run is quadratic.
const ADDR_RUN_CAP: usize = 128;

/// Redact whole base64 keys of 42 to 44 characters plus their `=` padding.
///
/// This is the strict, value-only rule the logs use on the way in: it never
/// touches hex keys, addresses or names, so log lines keep their wording. The
/// result is byte-identical to the former `[A-Za-z0-9+/]{42,44}=` regex.
pub fn redact_key_shaped(text: &str) -> String {
    let bytes = text.as_bytes();
    let is_key_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'+' || b == b'/';
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if is_key_byte(bytes[i]) {
            let start = i;
            while i < bytes.len() && is_key_byte(bytes[i]) {
                i += 1;
            }
            let run = i - start;
            if run >= 42 && i < bytes.len() && bytes[i] == b'=' {
                // The regex is leftmost-first and greedy: a longer run keeps
                // the bytes before its last 44 and redacts those plus `=`.
                out.push_str(&text[start..start + run.saturating_sub(44)]);
                out.push_str(REDACTED);
                i += 1;
            } else {
                out.push_str(&text[start..i]);
            }
        } else {
            let next = text[i..].chars().next().expect("a char starts here");
            out.push(next);
            i += next.len_utf8();
        }
    }
    out
}

/// How far an address reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrScope {
    /// Routable on the public internet.
    Public,
    /// A LAN address (RFC 1918, CGNAT).
    Private,
    /// The loopback interface.
    Loopback,
    /// A link-local address.
    LinkLocal,
    /// A multicast group.
    Multicast,
    /// The unspecified address.
    Unspecified,
}

/// Classify an address, or `None` when the text is not an address.
pub fn classify_addr(addr: &str) -> Option<AddrScope> {
    if let Ok(v4) = addr.parse::<Ipv4Addr>() {
        return Some(scope_v4(v4));
    }
    if let Ok(v6) = addr.parse::<Ipv6Addr>() {
        return Some(scope_v6(v6));
    }
    None
}

fn scope_v4(v: Ipv4Addr) -> AddrScope {
    let o = v.octets();
    match o[0] {
        0 => AddrScope::Unspecified,
        10 => AddrScope::Private,
        127 => AddrScope::Loopback,
        169 if o[1] == 254 => AddrScope::LinkLocal,
        172 if (16..=31).contains(&o[1]) => AddrScope::Private,
        192 if o[1] == 168 => AddrScope::Private,
        100 if (64..=127).contains(&o[1]) => AddrScope::Private,
        224..=239 => AddrScope::Multicast,
        255 => AddrScope::Multicast,
        _ => AddrScope::Public,
    }
}

fn scope_v6(v: Ipv6Addr) -> AddrScope {
    if let Some(v4) = v.to_ipv4() {
        return scope_v4(v4);
    }
    if v.is_loopback() {
        return AddrScope::Loopback;
    }
    if v.is_unspecified() {
        return AddrScope::Unspecified;
    }
    if v.is_multicast() {
        return AddrScope::Multicast;
    }
    let first = v.segments()[0];
    if first & 0xfe00 == 0xfc00 {
        return AddrScope::Private;
    }
    if first & 0xffc0 == 0xfe80 {
        return AddrScope::LinkLocal;
    }
    AddrScope::Public
}

/// One peer, with every spelling of its identity that the export uses.
#[derive(Debug, Clone)]
struct Peer {
    label: String,
    aliases: Vec<String>,
}

/// Redacts an export. Keep one instance per export so the same peer always
/// gets the same label inside it.
#[derive(Debug, Default)]
pub struct Redactor {
    peers: Vec<Peer>,
    next_peer: usize,
    hostname: Vec<char>,
}

impl Redactor {
    /// A redactor that knows nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register every spelling of one peer's identity (name, key prefix, pair
    /// id). They all get the same label, so one peer keeps one label across
    /// the whole export.
    pub fn note_peer(&mut self, aliases: &[&str]) -> String {
        let aliases: Vec<String> = aliases
            .iter()
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
            .collect();
        if aliases.is_empty() {
            return String::new();
        }
        if let Some(existing) = self
            .peers
            .iter_mut()
            .find(|p| p.aliases.iter().any(|a| aliases.contains(a)))
        {
            let label = existing.label.clone();
            for alias in aliases {
                if !existing.aliases.contains(&alias) {
                    existing.aliases.push(alias);
                }
            }
            return label;
        }
        self.next_peer += 1;
        let label = format!("peer-{}", self.next_peer);
        self.peers.push(Peer {
            label: label.clone(),
            aliases,
        });
        label
    }

    /// The machine name, so it never appears verbatim in an export.
    pub fn set_hostname(&mut self, hostname: &str) {
        let hostname = hostname.trim();
        if !hostname.is_empty() {
            self.hostname = hostname.chars().collect();
        }
    }

    /// Redact a whole export.
    pub fn redact(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&self.redact_line(line));
        }
        out
    }

    /// The label of a peer identity, registering it on first sight.
    fn peer_label(&mut self, value: &str) -> String {
        let value = value.trim();
        if value.is_empty() {
            return String::new();
        }
        // An already labelled peer keeps its label.
        if is_peer_label(value) {
            return value.to_string();
        }
        if let Some(p) = self
            .peers
            .iter()
            .find(|p| p.aliases.iter().any(|a| a == value))
        {
            return p.label.clone();
        }
        self.note_peer(&[value])
    }

    /// Redact one line, keeping its whitespace exactly as it was.
    fn redact_line(&mut self, line: &str) -> String {
        let line = self.replace_aliases(line);
        let line = self.redact_name_fields(&line);
        let (tokens, mut gaps) = split_tokens(&line);
        if tokens.is_empty() {
            return String::new();
        }
        let n = tokens.len();
        let mut replacement: Vec<Option<String>> = vec![None; n];
        let mut pending_code = false;
        let mut i = 0;
        while i < n {
            // "key = value", the shape a WireGuard config dump uses.
            if tokens[i] == "=" && i > 0 && i + 1 < n && !tokens[i - 1].contains('=') {
                let key = tokens[i - 1].clone();
                let value = tokens[i + 1].clone();
                let redacted = self.redact_pair(&key, &value);
                replacement[i - 1] = Some(key);
                replacement[i] = Some("=".to_string());
                replacement[i + 1] = Some(redacted);
                i += 2;
                continue;
            }
            if pending_code {
                pending_code = false;
                if is_code_digits(&tokens[i]) {
                    replacement[i] = Some(CODE.to_string());
                    i += 1;
                    continue;
                }
            }
            // A whole token that is a registered peer identity.
            if let Some(label) = self.alias_of(&tokens[i]) {
                replacement[i] = Some(label.to_string());
                i += 1;
                continue;
            }
            // A bare key token wins over `key=value` parsing: its trailing
            // padding is not a field separator.
            let token_chars: Vec<char> = tokens[i].chars().collect();
            if let Some(len) = self.key_shape_len(&token_chars, 0) {
                if len == token_chars.len() {
                    replacement[i] = Some(REDACTED.to_string());
                    i += 1;
                    continue;
                }
            }
            match tokens[i].split_once('=') {
                Some((key, value)) if !key.is_empty() && !key.contains(' ') => {
                    // A key printed in space-separated groups spans several
                    // tokens; the first group follows `key=`.
                    if let Some(span) = space_separated_hex_span(value, &tokens, &gaps, i + 1) {
                        replacement[i] = Some(format!("{key}={REDACTED}"));
                        for k in i + 1..=i + span {
                            replacement[k] = Some(String::new());
                            gaps[k] = String::new();
                        }
                        i += span + 1;
                        continue;
                    }
                    let redacted = self.redact_pair(key, value);
                    replacement[i] = Some(format!("{key}={redacted}"));
                }
                _ => {
                    if is_code_marker(&tokens[i]) {
                        pending_code = true;
                    }
                    let scanned = self.scan_values(&tokens[i]);
                    replacement[i] = Some(scanned);
                }
            }
            i += 1;
        }
        let mut out = String::with_capacity(line.len());
        for (i, token) in tokens.iter().enumerate() {
            out.push_str(&gaps[i]);
            match &replacement[i] {
                Some(r) => out.push_str(r),
                None => out.push_str(token),
            }
        }
        out
    }

    /// The label of a token that is exactly one registered identity.
    fn alias_of(&self, token: &str) -> Option<&str> {
        self.peers
            .iter()
            .find(|p| p.aliases.iter().any(|a| a == token))
            .map(|p| p.label.as_str())
    }

    /// A peer name can contain spaces, so it never survives a token split.
    /// A short name has to be a whole token, or it would hit half the words in
    /// the export. Key shapes win: an alias is never substituted inside a key,
    /// which would leave the key's tail visible.
    fn replace_aliases(&self, line: &str) -> String {
        if self.peers.is_empty() {
            return line.to_string();
        }
        let chars: Vec<char> = line.chars().collect();
        let mut out = String::with_capacity(line.len());
        let mut i = 0;
        while i < chars.len() {
            // A key shape that starts here is copied whole, so a key prefix
            // alias inside a longer key cannot match.
            if let Some(len) = self.key_shape_len(&chars, i) {
                for c in &chars[i..i + len] {
                    out.push(*c);
                }
                i += len;
                continue;
            }
            if let Some((len, label)) = self.alias_at(&chars, i) {
                out.push_str(label);
                i += len;
                continue;
            }
            // A long base64 run is not a key (a key has its padding) and no
            // token-bounded alias can start inside it: copy it whole.
            let run = b64_run(&chars, i);
            if run > 44 {
                for c in &chars[i..i + run] {
                    out.push(*c);
                }
                i += run;
                continue;
            }
            out.push(chars[i]);
            i += 1;
        }
        out
    }

    /// The length of a whole hex or base64 key at `i`, if one starts there.
    fn key_shape_len(&self, chars: &[char], i: usize) -> Option<usize> {
        if let Some((len, _)) = self.match_hex_key(chars, i) {
            return Some(len);
        }
        if let Some((len, _)) = self.match_b64_key(chars, i) {
            return Some(len);
        }
        None
    }

    /// The longest registered identity at `i`, only when it is not part of a
    /// longer alphanumeric/base64/hex run.
    fn alias_at<'a>(&'a self, chars: &[char], i: usize) -> Option<(usize, &'a str)> {
        if i > 0 && is_run_char(chars[i - 1]) {
            return None;
        }
        let mut best: Option<(usize, &str)> = None;
        for p in &self.peers {
            for alias in &p.aliases {
                let len = alias.chars().count();
                if len < 8 && !alias.contains(' ') {
                    continue;
                }
                if best.is_some_and(|(best_len, _)| len <= best_len) {
                    continue;
                }
                let end = i + len;
                if end > chars.len()
                    || (end < chars.len() && is_run_char(chars[end]))
                    || !matches_at(chars, i, alias)
                {
                    continue;
                }
                best = Some((len, &p.label));
            }
        }
        best
    }

    /// A peer, SSID or device name can contain spaces. Consume its whole value
    /// (quoted or to the end of the record line) so no word of it survives.
    fn redact_name_fields(&mut self, line: &str) -> String {
        let chars: Vec<char> = line.chars().collect();
        let mut out = String::with_capacity(line.len());
        let mut i = 0;
        while i < chars.len() {
            if i == 0 || chars[i - 1].is_whitespace() {
                if let Some((key_end, value_start)) = name_assignment_at(&chars, i) {
                    let key: String = chars[i..key_end].iter().collect();
                    if let Some(kind) = name_field(&normalize_key(&key)) {
                        let (value_end, next) = name_value_span(&chars, value_start);
                        let raw: String = chars[value_start..value_end].iter().collect();
                        let value = trim_quotes(raw.trim());
                        let replacement = match kind {
                            NameField::Peer => self.peer_label(value),
                            NameField::Ssid => SSID.to_string(),
                            NameField::Device => DEVICE.to_string(),
                        };
                        for c in &chars[i..value_start] {
                            out.push(*c);
                        }
                        out.push_str(&replacement);
                        i = next;
                        continue;
                    }
                }
            }
            out.push(chars[i]);
            i += 1;
        }
        out
    }

    /// Redact one `key=value` pair.
    fn redact_pair(&mut self, key: &str, value: &str) -> String {
        let normalized = normalize_key(key);
        match normalized.as_str() {
            // Key material, whatever the encoding.
            "privatekey" | "presharedkey" | "publickey" | "psk" | "key" => REDACTED.to_string(),
            // The peer's identity: one label per peer.
            "peerid" | "peername" | "pairedname" | "peerkeyprefix" | "pairid" => {
                let label = self.peer_label(value);
                if label.is_empty() {
                    String::new()
                } else {
                    label
                }
            }
            "ssid" | "wifissid" => SSID.to_string(),
            "device" | "devicename" | "phonename" => DEVICE.to_string(),
            "host" | "hostname" | "machine" => HOST.to_string(),
            "mac" | "macaddress" | "hwaddr" => MAC.to_string(),
            "code" | "pairingcode" => CODE.to_string(),
            // Everything else, including the peer endpoint: by value. The
            // value is scanned on its own, so a value that is itself a key
            // cannot slip through.
            _ => self.scan_values(value),
        }
    }

    /// Replace every recognizable value inside one token.
    fn scan_values(&mut self, text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < chars.len() {
            if let Some((len, replacement)) = self.match_at(&chars, i) {
                out.push_str(&replacement);
                i += len;
                continue;
            }
            // A long run of base64 characters can never be a key: a key is
            // followed by its padding. Skip the whole run instead of
            // re-reading it at every position.
            let run = b64_run(&chars, i);
            if run > 44 {
                for c in &chars[i..i + run] {
                    out.push(*c);
                }
                i += run;
                continue;
            }
            out.push(chars[i]);
            i += 1;
        }
        out
    }

    /// Match one value at `i`, most specific first.
    fn match_at(&self, chars: &[char], i: usize) -> Option<(usize, String)> {
        if let Some((len, _)) = self.match_home_path(chars, i) {
            return Some((len, HOME.to_string()));
        }
        if let Some((len, _)) = self.match_hostname(chars, i) {
            return Some((len, HOST.to_string()));
        }
        if let Some((len, replacement)) = self.match_ipv4(chars, i) {
            return Some((len, replacement));
        }
        if let Some((len, replacement)) = self.match_ipv6_or_mac(chars, i) {
            return Some((len, replacement));
        }
        if let Some((len, replacement)) = self.match_separated_hex_key(chars, i) {
            return Some((len, replacement));
        }
        if let Some((len, replacement)) = self.match_separated_b64_key(chars, i) {
            return Some((len, replacement));
        }
        if let Some((len, replacement)) = self.match_hex_key(chars, i) {
            return Some((len, replacement));
        }
        if let Some((len, replacement)) = self.match_b64_key(chars, i) {
            return Some((len, replacement));
        }
        None
    }

    /// A whole 64-digit hex key split into even groups by `:` or a space.
    ///
    /// `aa:bb:...` and `aaaaaaaa bbbbbbbb ...` are the same key in another
    /// costume; the groups must add up to exactly 64 hex digits.
    fn match_separated_hex_key(&self, chars: &[char], i: usize) -> Option<(usize, String)> {
        if !chars[i].is_ascii_hexdigit() {
            return None;
        }
        let mut j = i;
        let mut digits = 0;
        loop {
            let start = j;
            while j < chars.len() && chars[j].is_ascii_hexdigit() {
                j += 1;
            }
            let len = j - start;
            if len == 0 || !len.is_multiple_of(2) || len > 16 {
                return None;
            }
            digits += len;
            if digits == 64 {
                return Some((j - i, REDACTED.to_string()));
            }
            if digits > 64 || j >= chars.len() {
                return None;
            }
            if chars[j] != ':' && chars[j] != ' ' {
                return None;
            }
            if j + 1 >= chars.len() || !chars[j + 1].is_ascii_hexdigit() {
                return None;
            }
            j += 1;
        }
    }

    /// A 44-character base64 key split into groups by `:` or a space, with
    /// the padding after the last group.
    fn match_separated_b64_key(&self, chars: &[char], i: usize) -> Option<(usize, String)> {
        if !is_b64(chars[i]) {
            return None;
        }
        let mut j = i;
        let mut total = 0;
        loop {
            let start = j;
            while j < chars.len() && is_b64(chars[j]) {
                j += 1;
            }
            let len = j - start;
            if len == 0 {
                return None;
            }
            total += len;
            if total == 44 {
                if j < chars.len() && chars[j] == '=' {
                    return Some((j + 1 - i, REDACTED.to_string()));
                }
                return None;
            }
            if total > 44 || j >= chars.len() {
                return None;
            }
            if chars[j] != ':' && chars[j] != ' ' {
                return None;
            }
            if j + 1 >= chars.len() || !is_b64(chars[j + 1]) {
                return None;
            }
            j += 1;
        }
    }

    /// `/home/<user>` and `/data/user/0` become `~`. Only the prefix is
    /// consumed: the rest of the path is scanned normally, so an address that
    /// follows the home directory is still redacted.
    fn match_home_path(&self, chars: &[char], i: usize) -> Option<(usize, String)> {
        let home: Vec<char> = "/home/".chars().collect();
        if chars[i..].starts_with(&home[..]) {
            let mut j = i + home.len();
            while j < chars.len()
                && (chars[j].is_ascii_alphanumeric()
                    || chars[j] == '.'
                    || chars[j] == '_'
                    || chars[j] == '-')
            {
                j += 1;
            }
            return Some((j - i, HOME.to_string()));
        }
        let data: Vec<char> = "/data/user/0".chars().collect();
        if chars[i..].starts_with(&data[..]) {
            let mut j = i + data.len();
            while j < chars.len() && chars[j] != '/' {
                j += 1;
            }
            return Some((j - i, HOME.to_string()));
        }
        None
    }

    /// The machine name, if the export knows it.
    fn match_hostname(&self, chars: &[char], i: usize) -> Option<(usize, String)> {
        if self.hostname.is_empty() {
            return None;
        }
        if chars[i..].starts_with(self.hostname.as_slice()) {
            Some((self.hostname.len(), HOST.to_string()))
        } else {
            None
        }
    }

    /// A dotted-quad address.
    fn match_ipv4(&self, chars: &[char], i: usize) -> Option<(usize, String)> {
        if !chars[i].is_ascii_digit() {
            return None;
        }
        let mut j = i;
        while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.') {
            j += 1;
        }
        while j > i && chars[j - 1] == '.' {
            j -= 1;
        }
        let candidate: String = chars[i..j].iter().collect();
        let addr = candidate.parse::<Ipv4Addr>().ok()?;
        let replacement = match scope_v4(addr) {
            AddrScope::Public => PUBLIC_IP.to_string(),
            _ => candidate.clone(),
        };
        Some((j - i, replacement))
    }

    /// A colon-separated IPv6 address, or a MAC address.
    fn match_ipv6_or_mac(&self, chars: &[char], i: usize) -> Option<(usize, String)> {
        if !chars[i].is_ascii_hexdigit() && chars[i] != ':' {
            return None;
        }
        let mut j = i;
        while j < chars.len()
            && j - i < ADDR_RUN_CAP
            && (chars[j].is_ascii_hexdigit() || chars[j] == ':' || chars[j] == '.')
        {
            j += 1;
        }
        // A run longer than the cap is not an address: stop before parsing
        // every prefix of a huge run, which would be quadratic.
        if j < chars.len() && (chars[j].is_ascii_hexdigit() || chars[j] == ':' || chars[j] == '.') {
            return None;
        }
        // An address with a port after it: the port is not part of the
        // address, so the address is everything before the last colon.
        if let Some(split) = last_colon(chars, i, j) {
            let suffix: String = chars[split + 1..j].iter().collect();
            if !suffix.is_empty() && suffix.len() <= 5 && suffix.chars().all(|c| c.is_ascii_digit())
            {
                let candidate: String = chars[i..split].iter().collect();
                if let Ok(addr) = candidate.parse::<Ipv6Addr>() {
                    let replacement = match scope_v6(addr) {
                        AddrScope::Public => PUBLIC_IP.to_string(),
                        _ => candidate.clone(),
                    };
                    return Some((split - i, replacement));
                }
            }
        }
        let end = j;
        // The longest prefix that parses as an address wins.
        let mut e = end;
        while e > i {
            let candidate: String = chars[i..e].iter().collect();
            if let Ok(addr) = candidate.parse::<Ipv6Addr>() {
                let replacement = match scope_v6(addr) {
                    AddrScope::Public => PUBLIC_IP.to_string(),
                    _ => candidate.clone(),
                };
                return Some((e - i, replacement));
            }
            e -= 1;
        }
        if let Some(len) = mac_len(chars, i) {
            return Some((len, MAC.to_string()));
        }
        None
    }

    /// A 64-character hex key (the WireGuard UAPI encoding).
    fn match_hex_key(&self, chars: &[char], i: usize) -> Option<(usize, String)> {
        if !chars[i].is_ascii_hexdigit() {
            return None;
        }
        let mut j = i;
        while j < chars.len() && chars[j].is_ascii_hexdigit() {
            j += 1;
        }
        if j - i < 64 {
            return None;
        }
        Some((j - i, REDACTED.to_string()))
    }

    /// A 44-character base64 key with its padding.
    fn match_b64_key(&self, chars: &[char], i: usize) -> Option<(usize, String)> {
        if !is_b64(chars[i]) {
            return None;
        }
        let len = b64_run(chars, i);
        if !(42..=44).contains(&len) {
            return None;
        }
        let end = i + len;
        if end >= chars.len() || chars[end] != '=' {
            return None;
        }
        Some((len + 1, REDACTED.to_string()))
    }
}

/// Redact one export. Use a `Redactor` when the export spans several pieces
/// and the same peer must keep the same label in all of them.
pub fn redact_export(text: &str) -> String {
    redact_export_with(text, &[], "")
}

/// The one entry point for every platform's export: the text, every peer's
/// aliases (name, key prefix, pair id) and the machine name, if known.
///
/// Each inner slice is one peer's spellings. The labels are assigned in
/// registration order, so the first peer is `peer-1` wherever it first
/// appears. An empty host name means "not known"; it is not redacted.
pub fn redact_export_with(text: &str, peers: &[&[&str]], hostname: &str) -> String {
    let mut redactor = Redactor::new();
    redactor.set_hostname(hostname);
    for aliases in peers {
        redactor.note_peer(aliases);
    }
    redactor.redact(text)
}

/// One name field whose value may contain spaces.
#[derive(Clone, Copy)]
enum NameField {
    Peer,
    Ssid,
    Device,
}

/// The name field a normalized key introduces, if any.
fn name_field(normalized: &str) -> Option<NameField> {
    match normalized {
        "peername" | "pairedname" => Some(NameField::Peer),
        "ssid" | "wifissid" => Some(NameField::Ssid),
        "device" | "devicename" | "phonename" => Some(NameField::Device),
        _ => None,
    }
}

/// A key with its case and punctuation removed, for field matching.
fn normalize_key(key: &str) -> String {
    key.to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// Parse `key=value` or `key = value` at `start`: the key end and the first
/// character of the value.
fn name_assignment_at(chars: &[char], start: usize) -> Option<(usize, usize)> {
    let mut key_end = start;
    while key_end < chars.len() && !chars[key_end].is_whitespace() && chars[key_end] != '=' {
        key_end += 1;
    }
    if key_end == start {
        return None;
    }
    let mut eq = key_end;
    while eq < chars.len() && chars[eq].is_whitespace() {
        eq += 1;
    }
    if eq >= chars.len() || chars[eq] != '=' {
        return None;
    }
    let mut value_start = eq + 1;
    while value_start < chars.len() && chars[value_start].is_whitespace() {
        value_start += 1;
    }
    Some((key_end, value_start))
}

/// The value's end and the index to resume at. A simply quoted value ends at
/// its closing quote; an unquoted one runs to the end of the record line.
fn name_value_span(chars: &[char], value_start: usize) -> (usize, usize) {
    if value_start >= chars.len() {
        return (chars.len(), chars.len());
    }
    let quote = chars[value_start];
    if quote == '"' || quote == '\'' {
        if let Some(rel) = chars[value_start + 1..].iter().position(|c| *c == quote) {
            let end = value_start + 1 + rel + 1;
            return (end, end);
        }
    }
    (chars.len(), chars.len())
}

/// Remove one pair of matching single or double quotes.
fn trim_quotes(value: &str) -> &str {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        return inner;
    }
    if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        return inner;
    }
    value
}

/// Whether `text` starts at `chars[i]`.
fn matches_at(chars: &[char], i: usize, text: &str) -> bool {
    for (k, c) in (i..).zip(text.chars()) {
        if k >= chars.len() || chars[k] != c {
            return false;
        }
    }
    true
}

/// A character that continues an identifier, a base64 token or a hex run.
fn is_run_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '_' || c == '-'
}

/// Split a line into its tokens and the whitespace in front of each token.
fn split_tokens(line: &str) -> (Vec<String>, Vec<String>) {
    let mut tokens: Vec<String> = Vec::new();
    let mut gaps: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut gap = String::new();
    for c in line.chars() {
        if c.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
                gaps.push(std::mem::take(&mut gap));
            }
            gap.push(c);
            continue;
        }
        current.push(c);
    }
    if !current.is_empty() {
        tokens.push(current);
        gaps.push(gap);
    }
    (tokens, gaps)
}

fn is_b64(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '+' || c == '/'
}

/// A group of a space-separated hex key: an even number of hex digits, at
/// most 16 (the longest natural grouping).
fn hex_group_len(group: &str) -> Option<usize> {
    let len = group.chars().count();
    if len == 0 || !len.is_multiple_of(2) || len > 16 {
        return None;
    }
    if !group.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(len)
}

/// How many tokens after `next` complete a 64-digit hex key that starts with
/// `first`, when the groups are separated by single spaces.
fn space_separated_hex_span(
    first: &str,
    tokens: &[String],
    gaps: &[String],
    next: usize,
) -> Option<usize> {
    let mut total = hex_group_len(first)?;
    let mut span = 0;
    let mut k = next;
    while total < 64 {
        if k >= tokens.len() || gaps[k] != " " {
            return None;
        }
        total += hex_group_len(&tokens[k])?;
        span += 1;
        k += 1;
    }
    (total == 64).then_some(span)
}

/// The length of the base64 run at `i`.
fn b64_run(chars: &[char], i: usize) -> usize {
    let mut j = i;
    while j < chars.len() && is_b64(chars[j]) {
        j += 1;
    }
    j - i
}

/// The last colon between `i` and `end`, if there is one.
fn last_colon(chars: &[char], i: usize, end: usize) -> Option<usize> {
    (i..end).rev().find(|&k| chars[k] == ':')
}

/// The length of a MAC address at `i`, if there is one.
fn mac_len(chars: &[char], i: usize) -> Option<usize> {
    let mut j = i;
    let mut groups = 0;
    while groups < 6 {
        if j + 1 >= chars.len()
            || !chars[j].is_ascii_hexdigit()
            || !chars[j + 1].is_ascii_hexdigit()
        {
            return None;
        }
        j += 2;
        groups += 1;
        if groups == 6 {
            break;
        }
        if j >= chars.len() || (chars[j] != ':' && chars[j] != '-') {
            return None;
        }
        j += 1;
    }
    Some(j - i)
}

/// A word that introduces a pairing code.
fn is_code_marker(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "code" | "code:" | "code=" | "pairing" | "pairing:" | "pairingcode" | "pairing-code"
    )
}

/// Digits that are a pairing code.
fn is_code_digits(token: &str) -> bool {
    let len = token.chars().count();
    (4..=6).contains(&len) && token.chars().all(|c| c.is_ascii_digit())
}

/// A label this redactor gave to a peer.
fn is_peer_label(value: &str) -> bool {
    match value.strip_prefix("peer-") {
        Some(rest) => !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{redact_key_shaped, REDACTED};

    #[test]
    fn a_base64_key_with_padding_is_redacted() {
        let key = format!("{}=", "A".repeat(43));
        assert_eq!(
            redact_key_shaped(&format!("peer {key} denied")),
            "peer [redacted] denied"
        );
    }

    #[test]
    fn the_42_to_44_byte_boundaries_are_keys_and_41_is_not() {
        for len in [42, 43, 44] {
            let key = format!("{}=", "B".repeat(len));
            assert_eq!(redact_key_shaped(&key), "[redacted]", "len {len}");
        }
        let text = format!("{}=", "C".repeat(41));
        assert_eq!(redact_key_shaped(&text), text);
    }

    #[test]
    fn a_longer_run_keeps_the_prefix_the_regex_left() {
        let text = format!("{}=", "D".repeat(46));
        assert_eq!(redact_key_shaped(&text), format!("DD{REDACTED}"));
    }

    #[test]
    fn other_values_and_non_ascii_text_are_kept_byte_for_byte() {
        let text = "ssid=Cafe \u{fc} 10.0.0.1 peer=10.0.0.2 /dev/null";
        assert_eq!(redact_key_shaped(text), text);
    }
}
